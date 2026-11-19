#![cfg(windows)]

use flowscale_abi::ScalingType;
use flowscale_gpu::{ComputePipeline, D3d11Context};
use flowscale_gpu::FpsOverlay;
use flowscale_overlay::CursorSnapshot;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R32G32B32A32_FLOAT;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R16G16B16A16_FLOAT;

const SIDE: u32 = 8;

#[test]
fn hdr_overlay_preserves_exact_finite_black() {
    let Ok(ctx) = D3d11Context::new(0) else {
        eprintln!("HDR overlay audit requires a D3D11 adapter");
        return;
    };
    let format = DXGI_FORMAT_R16G16B16A16_FLOAT;
    let (texture, _, uav) = ctx.create_rw_texture(1, 1, format).unwrap();
    unsafe { ctx.context.ClearUnorderedAccessViewFloat(&uav, &[1.0; 4]) };
    let cursor = CursorSnapshot {
        visible: true, screen_x: 0, screen_y: 0, hotspot_x: 0, hotspot_y: 0,
        width: 1, height: 1, rgba: vec![0, 0, 0, 255],
    };
    FpsOverlay::new(18.0).unwrap()
        .draw_cursor(&texture, format, &cursor, 0.0, 0.0, 1.0).unwrap();
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe { texture.GetDesc(&mut desc) };
    desc.Usage = D3D11_USAGE_STAGING;
    desc.BindFlags = 0;
    desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
    let mut staging = None;
    unsafe { ctx.device.CreateTexture2D(&desc, None, Some(&mut staging)).unwrap() };
    let staging = staging.unwrap();
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    let pixel = unsafe {
        ctx.context.CopyResource(&staging, &texture);
        ctx.context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).unwrap();
        let pixel = mapped.pData.cast::<[u16; 4]>().read_unaligned();
        ctx.context.Unmap(&staging, 0);
        pixel
    };
    assert_eq!(pixel, [0, 0, 0, 0x3c00]);
}

fn upload(
    ctx: &D3d11Context,
    pixels: &[[f32; 4]],
) -> windows::Win32::Graphics::Direct3D11::ID3D11ShaderResourceView {
    let (texture, srv, _) = ctx
        .create_rw_texture(SIDE, SIDE, DXGI_FORMAT_R32G32B32A32_FLOAT)
        .unwrap();
    unsafe {
        ctx.context
            .UpdateSubresource(&texture, 0, None, pixels.as_ptr().cast(), SIDE * 16, 0);
    }
    srv
}

fn readback(
    ctx: &D3d11Context,
    texture: &windows::Win32::Graphics::Direct3D11::ID3D11Texture2D,
) -> Vec<[f32; 4]> {
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe { texture.GetDesc(&mut desc) };
    desc.Usage = D3D11_USAGE_STAGING;
    desc.BindFlags = 0;
    desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
    let mut staging = None;
    unsafe {
        ctx.device
            .CreateTexture2D(&desc, None, Some(&mut staging))
            .unwrap();
    }
    let staging = staging.unwrap();
    unsafe {
        ctx.context.CopyResource(&staging, texture);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        ctx.context
            .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .unwrap();
        let mut pixels = Vec::with_capacity((SIDE * SIDE) as usize);
        for y in 0..SIDE {
            let row = mapped
                .pData
                .cast::<u8>()
                .add(y as usize * mapped.RowPitch as usize);
            for x in 0..SIDE {
                pixels.push(row.cast::<[f32; 4]>().add(x as usize).read_unaligned());
            }
        }
        ctx.context.Unmap(&staging, 0);
        pixels
    }
}

#[test]
fn upscale_kernels_preserve_negative_scrgb_channels() {
    let Ok(ctx) = D3d11Context::new(0) else {
        eprintln!("scRGB shader audit requires a D3D11 adapter");
        return;
    };
    let pipeline = ComputePipeline::new(&ctx, SIDE, SIDE).unwrap();
    let pixels: Vec<[f32; 4]> = (0..SIDE)
        .flat_map(|y| {
            (0..SIDE).map(move |x| {
                [
                    -1.0 - x as f32 * 0.05,
                    -2.0 - y as f32 * 0.05,
                    -4.0 - (x + y) as f32 * 0.025,
                    1.0,
                ]
            })
        })
        .collect();
    let input = upload(&ctx, &pixels);
    let (output, _, output_uav) = ctx
        .create_rw_texture(SIDE, SIDE, DXGI_FORMAT_R32G32B32A32_FLOAT)
        .unwrap();

    for scaling_type in [
        ScalingType::SharpBilinear,
        ScalingType::BicubicCas,
        ScalingType::Fsr,
        ScalingType::Nis,
        ScalingType::Ls1,
        ScalingType::Anime4k,
        ScalingType::Sgsr,
    ] {
        pipeline
            .dispatch_upscale_ex(
                &ctx,
                scaling_type,
                &input,
                &output_uav,
                SIDE,
                SIDE,
                SIDE,
                SIDE,
                [0, 0, SIDE, SIDE],
                4,
                4,
                false,
            )
            .unwrap();
        let result = readback(&ctx, &output);
        assert!(
            result
                .iter()
                .all(|rgba| rgba[..3].iter().all(|channel| *channel < 0.0)),
            "{scaling_type:?} clipped negative scRGB"
        );
    }
}

#[test]
fn anime4k_vrs_flag_reduces_restoration_iterations() {
    let Ok(ctx) = D3d11Context::new(0) else {
        eprintln!("Anime4K VRS audit requires a D3D11 adapter");
        return;
    };
    let pipeline = ComputePipeline::new(&ctx, SIDE, SIDE).unwrap();
    let pixels: Vec<[f32; 4]> = (0..SIDE)
        .flat_map(|_| {
            (0..SIDE).map(move |x| {
                let value = if x < SIDE / 2 { 0.0 } else { 1.0 };
                [value, value, value, 1.0]
            })
        })
        .collect();
    let input = upload(&ctx, &pixels);
    let (output, _, output_uav) = ctx
        .create_rw_texture(SIDE, SIDE, DXGI_FORMAT_R32G32B32A32_FLOAT)
        .unwrap();
    let render = |vrs| {
        pipeline
            .dispatch_upscale_ex(
                &ctx,
                ScalingType::Anime4k,
                &input,
                &output_uav,
                SIDE,
                SIDE,
                SIDE,
                SIDE,
                [0, 0, SIDE, SIDE],
                4,
                4,
                vrs,
            )
            .unwrap();
        readback(&ctx, &output)
    };
    let full = render(false);
    let vrs = render(true);
    assert!(
        full.iter()
            .zip(vrs)
            .any(|(a, b)| (a[0] - b[0]).abs() > 1e-4)
    );
}
