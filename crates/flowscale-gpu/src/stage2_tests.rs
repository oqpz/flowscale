use super::*;
use flowscale_abi::FrameGenerationSize;
use windows::Win32::Graphics::Direct3D11::{D3D11_USAGE_STAGING,D3D11_CPU_ACCESS_READ,D3D11_MAP_READ};

fn upload(ctx: &D3d11Context, w: u32, h: u32, pixels: &[u32]) -> ID3D11ShaderResourceView {
    let (tex, srv, _) = ctx.create_rw_texture(w, h, DXGI_FORMAT_B8G8R8A8_UNORM).unwrap();
    unsafe { ctx.context.UpdateSubresource(&tex, 0, None, pixels.as_ptr().cast(), w * 4, 0) };
    srv
}

pub(super) fn read<T: Copy>(ctx: &D3d11Context, texture: &ID3D11Texture2D) -> Vec<T> {
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe { texture.GetDesc(&mut desc) };
    desc.Usage = D3D11_USAGE_STAGING;
    desc.BindFlags = 0;
    desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
    let mut staging = None;
    unsafe { ctx.device.CreateTexture2D(&desc, None, Some(&mut staging)).unwrap() };
    let staging = staging.unwrap();
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    let mut result = Vec::new();
    unsafe {
        ctx.context.CopyResource(&staging, texture);
        ctx.context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).unwrap();
        for y in 0..desc.Height as usize {
            let row = mapped.pData.cast::<u8>().add(y * mapped.RowPitch as usize).cast::<T>();
            for x in 0..desc.Width as usize { result.push(row.add(x).read_unaligned()); }
        }
        ctx.context.Unmap(&staging, 0);
    }
    result
}

fn scene(x: f32, y: f32) -> f32 {
    (0.45 + 0.18 * (x * 0.089 + y * 0.023).sin()
        + 0.13 * (y * 0.097 - x * 0.031).cos()
        + 0.09 * (x * 0.33 + y * 0.17).sin()).clamp(0.0, 1.0)
}

fn image(w: u32, h: u32, dx: f32, dy: f32, opacity: f32) -> Vec<u32> {
    (0..w*h).map(|i| {
        let (x, y) = (i % w, i / w);
        let mut v = scene(x as f32 - dx, y as f32 - dy);
        if (24..184).contains(&x) && (24..88).contains(&y) {
            v = opacity * 0.06 + (1.0 - opacity) * v;
            if (40..80).contains(&x) && ((40..43).contains(&y) || (52..55).contains(&y) || (64..67).contains(&y)) { v = if opacity > 0.0 { 0.97 } else { v }; }
        }
        let b = (v * 255.0).round() as u32;
        0xff00_0000 | (b * 0x0001_0101)
    }).collect()
}

#[test]
fn stage2_translation_range_and_grid_phases() {
    let ctx = D3d11Context::new(0).expect("hardware GPU required for Stage2 quality gate");
    let mut failures = Vec::new();
    let mut cases = 0;
    for (w,h) in [(384,320), (389,323)] {
        for pct in [25,50,75,100] {
            let pipeline = ComputePipeline::new_with_flow_scale(&ctx,w,h,pct).unwrap();
            for radius in [search_radius_for_fg_size(FrameGenerationSize::Balanced),search_radius_for_fg_size(FrameGenerationSize::Performance),6] {
                let prev = upload(&ctx,w,h,&image(w,h,0.0,0.0,0.0));
                let mut shifts = Vec::new();
                for d in [0,1,7,8,15,24,32,48,64] {
                    shifts.extend([(d,0),(-d,0),(0,d),(0,-d)]);
                }
                shifts.extend([(48,48),(-48,-48),(64,64),(-64,-64),(68,0),(-68,0)]);
                for (dx,dy) in shifts {
                    let curr = upload(&ctx,w,h,&image(w,h,dx as f32,dy as f32,0.0));
                    pipeline.dispatch_motion_est(&ctx,&prev,&curr,w,h,radius).unwrap();
                    let vectors: Vec<[f32;2]> = read(&ctx,&pipeline.motion_tex);
                    let (mut good,mut count) = (0,0);
                    let mut bad_vectors = Vec::new();
                    for fy in 0..pipeline.flow_height { for fx in 0..pipeline.flow_width {
                        let x = (fx as f32 + 0.5) * w as f32 / pipeline.flow_width as f32;
                        let y = (fy as f32 + 0.5) * h as f32 / pipeline.flow_height as f32;
                        if x<12.0 || y<12.0 || x>=w as f32-12.0 || y>=h as f32-12.0
                            || x+(dx as f32)<12.0 || y+(dy as f32)<12.0 || x+(dx as f32)>=w as f32-12.0 || y+(dy as f32)>=h as f32-12.0 { continue; }
                        let mv = vectors[(fy*pipeline.flow_width+fx) as usize];
                        count += 1;
                        good += usize::from((mv[0]-dx as f32).hypot(mv[1]-dy as f32) <= 1.0);
                        if (mv[0]-dx as f32).hypot(mv[1]-dy as f32) > 1.0 && bad_vectors.len() < 4 { bad_vectors.push((fx,fy,mv)); }
                    }}
                    cases += 1;
                    if good * 100 < count * 95 { failures.push(format!("{w}x{h} {pct}% r{radius} ({dx},{dy}): {good}/{count} {bad_vectors:?}")); }
                }
            }
        }
    }
    eprintln!("Stage2 translations: {cases} cases, {} failures; first {:?}", failures.len(), &failures[..failures.len().min(20)]);
    assert!(failures.is_empty(), "translation quality below 95%: {} failures", failures.len());
}

#[test]
fn stage2_dark_pill_midpoint() {
    let ctx = D3d11Context::new(0).expect("hardware GPU required");
    let (w,h) = (512,384);
    let pipeline = ComputePipeline::new_with_flow_scale(&ctx,w,h,50).unwrap();
    let (out,_,uav) = ctx.create_rw_texture(w,h,DXGI_FORMAT_B8G8R8A8_UNORM).unwrap();
    let mut failures = Vec::new();
    for opacity in [0.45,0.55,0.70] { for (dx,dy) in [(24,0),(48,0),(0,24),(0,48),(48,48)] {
        let prev = upload(&ctx,w,h,&image(w,h,0.0,0.0,opacity));
        let curr = upload(&ctx,w,h,&image(w,h,dx as f32,dy as f32,opacity));
        let truth = image(w,h,dx as f32*0.5,dy as f32*0.5,opacity);
        pipeline.dispatch_motion_est(&ctx,&prev,&curr,w,h,8).unwrap();
        pipeline.dispatch_frame_interp_ex(&ctx,&prev,&curr,&uav,w,h,0.5,0.5).unwrap();
        let actual: Vec<u32> = read(&ctx,&out);
        let (mut bad,mut n) = (0,0);
        for y in 24..88 { for x in 24..184 {
            let i = (y*w+x) as usize;
            n += 1;
            bad += usize::from(((actual[i]&255) as i32 - (truth[i]&255) as i32).abs() > 32);
        }}
        eprintln!("pill {opacity} ({dx},{dy}): {:.3}% >32", bad as f32*100.0/n as f32);
        let oracle_mv = vec![[dx as f32,dy as f32]; (pipeline.flow_width*pipeline.flow_height) as usize];
        let oracle_confidence = vec![1.0f32; oracle_mv.len()];
        unsafe {
            ctx.context.UpdateSubresource(&pipeline.motion_tex, 0, None, oracle_mv.as_ptr().cast(), pipeline.flow_width * 8, 0);
            ctx.context.UpdateSubresource(&pipeline.occlusion_tex, 0, None, oracle_confidence.as_ptr().cast(), pipeline.flow_width * 4, 0);
        }
        pipeline.dispatch_frame_interp_ex(&ctx,&prev,&curr,&uav,w,h,0.5,0.5).unwrap();
        let oracle: Vec<u32> = read(&ctx,&out);
        let mut oracle_bad = 0;
        for y in 24..88 { for x in 24..184 {
            let i = (y*w+x) as usize;
            oracle_bad += usize::from(((oracle[i]&255) as i32-(truth[i]&255) as i32).abs()>32);
        }}
        eprintln!("pill oracle-flow {opacity} ({dx},{dy}): {:.3}% >32",oracle_bad as f32*100.0/n as f32);
        if bad*100 >= n { failures.push((opacity,dx,dy,bad,n)); }
    }}
    assert!(failures.is_empty(), "dark pill midpoint: {failures:?}");
}

#[test]
fn hud_protection_does_not_pin_matching_world_intensities() {
    let ctx = D3d11Context::new(0).expect("hardware GPU required");
    let (w, h) = (512, 384);
    let pipeline = ComputePipeline::new_with_flow_scale(&ctx, w, h, 50).unwrap();
    let (out, _, uav) = ctx.create_rw_texture(w, h, DXGI_FORMAT_B8G8R8A8_UNORM).unwrap();
    let prev = upload(&ctx, w, h, &image(w, h, 0.0, 0.0, 0.0));
    let curr = upload(&ctx, w, h, &image(w, h, 48.0, 48.0, 0.0));
    let truth = image(w, h, 24.0, 24.0, 0.0);
    let motion = vec![[48.0f32, 48.0]; (pipeline.flow_width * pipeline.flow_height) as usize];
    let confidence = vec![1.0f32; motion.len()];
    unsafe {
        ctx.context.UpdateSubresource(
            &pipeline.motion_tex, 0, None, motion.as_ptr().cast(), pipeline.flow_width * 8, 0,
        );
        ctx.context.UpdateSubresource(
            &pipeline.occlusion_tex, 0, None, confidence.as_ptr().cast(), pipeline.flow_width * 4, 0,
        );
    }
    for threshold in [0.0, 0.5] {
        pipeline.dispatch_frame_interp_ex(&ctx, &prev, &curr, &uav, w, h, 0.5, threshold).unwrap();
        let actual: Vec<u32> = read(&ctx, &out);
        let mut bad = 0;
        for y in 80..300 {
            for x in 80..400 {
                let i = (y * w + x) as usize;
                bad += usize::from(((actual[i] & 255) as i32 - (truth[i] & 255) as i32).abs() > 8);
            }
        }
        eprintln!("HUD threshold {threshold}: {bad}/70400 incorrect world pixels");
        assert!(bad < 71, "HUD protection pinned moving world pixels: {bad}");
    }
}

#[test]
fn stage2_all_group_phases_have_no_coherent_motion_bands() {
    let ctx = D3d11Context::new(0).expect("hardware GPU required");
    let (w,h) = (389,323);
    for pct in [25,50,75,100] { for radius in [8,5] {
    let pipeline = ComputePipeline::new_with_flow_scale(&ctx,w,h,pct).unwrap();
    let mx = (80*pipeline.flow_width).div_ceil(w);
    let my = (80*pipeline.flow_height).div_ceil(h);
    for py in 0..8 { for px in 0..8 {
        let x_phase = (px * w) as f32 / pipeline.flow_width as f32;
        let y_phase = (py * h) as f32 / pipeline.flow_height as f32;
        let prev = upload(&ctx, w, h, &image(w, h, x_phase, y_phase, 0.0));
        let curr = upload(&ctx, w, h, &image(w, h, x_phase + 48.0, y_phase + 48.0, 0.0));
        pipeline.dispatch_motion_est(&ctx,&prev,&curr,w,h,radius).unwrap();
        let vectors: Vec<[f32;2]> = read(&ctx,&pipeline.motion_tex);
        let (mut good,mut count) = (0,0);
        for fy in my..pipeline.flow_height-my {
            let mut run = 0;
            for fx in mx..pipeline.flow_width-mx {
                let mv = vectors[(fy*pipeline.flow_width+fx) as usize];
                let valid = (mv[0]-48.0).hypot(mv[1]-48.0) <= 1.0;
                good += usize::from(valid); count += 1;
                run = if valid { 0 } else { run+1 };
                assert!(run < 8, "coherent horizontal band at phase ({px},{py}), flow ({fx},{fy})");
            }
        }
        for fx in mx..pipeline.flow_width-mx {
            let mut run = 0;
            for fy in my..pipeline.flow_height-my {
                let mv = vectors[(fy*pipeline.flow_width+fx) as usize];
                run = if (mv[0]-48.0).hypot(mv[1]-48.0) <= 1.0 { 0 } else { run+1 };
                assert!(run < 8, "coherent vertical band at phase ({px},{py}), flow ({fx},{fy})");
            }
        }
        assert!(count>0 && good*100 >= count*95, "phase {pct}% r{radius} ({px},{py}): {good}/{count}");
    }}
    }}
}

#[test]
fn stage2_hdr_copy_history_interpolation_and_scale_preserve_fp16() {
    let ctx = D3d11Context::new(0).expect("hardware GPU required");
    let pipeline = ComputePipeline::new_with_flow_scale(&ctx,64,64,50).unwrap();
    let (capture,_,_) = ctx.create_rw_texture(64,64,DXGI_FORMAT_R16G16B16A16_FLOAT).unwrap();
    let pixels = vec![[0xb800u16,0x4000,0x3400,0x3c00];64*64]; // -0.5, 2.0, 0.25, 1.0
    unsafe { ctx.context.UpdateSubresource(&capture,0,None,pixels.as_ptr().cast(),64*8,0); }
    let (history,prev,_) = ctx.create_rw_texture(64,64,DXGI_FORMAT_R16G16B16A16_FLOAT).unwrap();
    unsafe { ctx.context.CopyResource(&history,&capture); }
    let curr = ctx.create_srv(&capture,DXGI_FORMAT_R16G16B16A16_FLOAT).unwrap();
    let (interpolated,interp,uav) = ctx.create_rw_texture(64,64,DXGI_FORMAT_R16G16B16A16_FLOAT).unwrap();
    let (scaled,_,out) = ctx.create_rw_texture(128,128,DXGI_FORMAT_R16G16B16A16_FLOAT).unwrap();
    pipeline.dispatch_motion_est(&ctx,&prev,&curr,64,64,8).unwrap();
    pipeline.dispatch_frame_interp_ex(&ctx,&prev,&curr,&uav,64,64,0.5,0.5).unwrap();
    pipeline.dispatch_upscale(&ctx,ScalingType::Nearest,&interp,&out,64,64,128,128,0).unwrap();
    for texture in [&history,&interpolated,&scaled] {
        let actual: Vec<[u16;4]> = read(&ctx,texture);
        assert!(actual.iter().all(|p| *p == pixels[0]), "FP16 negative/highlight channels changed");
    }
    let sdr = upload(&ctx,64,64,&vec![0xff80_8080;64*64]);
    pipeline.dispatch_sdr_to_scrgb(&ctx,&sdr,&uav,64,64,1.0).unwrap();
    let actual: Vec<[u16;4]> = read(&ctx,&interpolated);
    // sRGB 128/255 -> 0.21586 linear, fp16 approximately 0x32e8; neither 0.5 nor double-decoded 0.038.
    assert!((0x32e0..=0x32f0).contains(&actual[0][0]), "SDR transfer was not decoded once: {:x}", actual[0][0]);
}

#[test]
fn stage2_pending_timer_slots_are_never_overwritten() {
    let ctx = D3d11Context::new(0).expect("hardware GPU required");
    let mut ring = GpuTimerRing::new(&ctx.device).unwrap();
    for (i,slot) in ring.slots.iter_mut().enumerate() { slot.pending = true; slot.generation = i as u64+1; }
    assert!(!ring.ensure_active(&ctx.context));
    assert!(ring.slots.iter().all(|s| s.pending && !s.active));
    assert_eq!(ring.slots[0].generation,1);
    ring.slots[1].pending = false;
    assert!(ring.ensure_active(&ctx.context));
    assert_eq!(ring.write_idx,1);
    assert!(ring.slots[0].pending && ring.slots[2].pending);
    ring.finish_frame_and_poll(&ctx.context);
}

#[test]
fn stage2_occlusion_revealed_details_and_scene_cut_alpha_sweep() {
    let ctx = D3d11Context::new(0).expect("hardware GPU required");
    let side=192;
    let pipeline=ComputePipeline::new_with_flow_scale(&ctx,side,side,50).unwrap();
    let frame=|left: u32| -> Vec<u32> { (0..side*side).map(|i| {
        let (x,y)=(i%side,i/side);
        let gray=if (left..left+64).contains(&x) && (64..128).contains(&y) {
            160+((x-left)*13+(y-64)*7)%64
        } else if x==54 || x==122 { 255 } else { 24+((x*11)^(y*17))%64 };
        0xff00_0000 | (gray * 0x0001_0101)
    }).collect() };
    let prev=upload(&ctx,side,side,&frame(48));
    let curr=upload(&ctx,side,side,&frame(64));
    let (out,_,uav)=ctx.create_rw_texture(side,side,DXGI_FORMAT_B8G8R8A8_UNORM).unwrap();
    pipeline.dispatch_motion_est(&ctx,&prev,&curr,side,side,8).unwrap();
    for alpha in [0.25,0.49,0.50,0.51,0.75] {
        pipeline.dispatch_frame_interp_ex(&ctx,&prev,&curr,&uav,side,side,alpha,0.5).unwrap();
        let actual: Vec<u32>=read(&ctx,&out);
        let left=48.0+16.0*alpha;
        let mut holes = Vec::new();
        for y in 76..116 { for x in (left.ceil() as u32+6)..(left.floor() as u32+58) {
            let value = actual[(y * side + x) as usize] & 255;
            if value < 130 {
                holes.push((x, y, value));
            }
        }}
        assert!(holes.is_empty(), "foreground holes at phase {alpha}: {holes:?}");
        if alpha>=0.49 {
            let revealed=actual[(90*side+54) as usize]&255;
            assert!(revealed>=223,"newly revealed thin detail at phase {alpha}: {revealed}");
        }
    }
    let cut_prev=upload(&ctx,side,side,&vec![0xff20_2020;(side*side) as usize]);
    let cut_curr=upload(&ctx,side,side,&vec![0xffe0_e0e0;(side*side) as usize]);
    pipeline.dispatch_motion_est(&ctx,&cut_prev,&cut_curr,side,side,8).unwrap();
    for (alpha, expected) in [(0.0, 0xff20_2020), (1.0, 0xffe0_e0e0)] {
        pipeline.dispatch_frame_interp_ex(&ctx, &cut_prev, &cut_curr, &uav, side, side, alpha, 0.5).unwrap();
        assert!(read::<u32>(&ctx, &out).iter().all(|&pixel| pixel == expected));
    }
    let mut samples=Vec::new();
    for alpha in [0.25,0.49,0.50,0.51,0.75] {
        pipeline.dispatch_frame_interp_ex(&ctx,&cut_prev,&cut_curr,&uav,side,side,alpha,0.5).unwrap();
        samples.push(read::<u32>(&ctx,&out)[(90*side+90) as usize]&255);
    }
    assert!(samples.windows(2).all(|w|w[0]<=w[1]));
    assert!(samples[3]-samples[1]<=8,"scene-cut fallback jumps at alpha 0.5: {samples:?}");
}


#[test]
fn stage2_pan_preserves_separately_moving_world_edges() {
    let ctx = D3d11Context::new(0).expect("hardware GPU required");
    let (width, height) = (512, 384);
    let pipeline = ComputePipeline::new_with_flow_scale(&ctx, width, height, 50).unwrap();
    let (out, _, uav) = ctx.create_rw_texture(width, height, DXGI_FORMAT_B8G8R8A8_UNORM).unwrap();
    let frame = |pan_x: i32, pan_y: i32, left: i32, top: i32| {
        let pixels: Vec<_> = (0..width * height).map(|i| {
            let x = (i % width) as i32;
            let y = (i / width) as i32;
            let rx = x - left;
            let ry = y - top;
            if (0..64).contains(&rx) && (0..64).contains(&ry) {
                let detail = (scene(rx as f32 + 170.0, ry as f32 + 39.0) * 100.0) as u32;
                0xffc0_4010 + (detail << 8)
            } else {
                let gray = (scene((x - pan_x) as f32, (y - pan_y) as f32) * 255.0) as u32;
                0xff00_0000 | (gray * 0x0001_0101)
            }
        }).collect();
        upload(&ctx, width, height, &pixels)
    };
    let prev = frame(0, 0, 110, 132);
    let curr = frame(32, 24, 130, 122);
    pipeline.dispatch_motion_est(&ctx, &prev, &curr, width, height, 8).unwrap();
    pipeline.dispatch_frame_interp_ex(&ctx, &prev, &curr, &uav, width, height, 0.5, 0.5).unwrap();
    let pixels: Vec<u32> = read(&ctx, &out);
    let red_edge = |left: u32, top: u32| -> usize {
        (top..top + 42).flat_map(|y| (left..left + 5).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let px = pixels[(y * width + x) as usize];
                ((px >> 16) & 255) > (px & 255) + 80
            })
            .count()
    };
    let inside = red_edge(122, 138) + red_edge(177, 138);
    let outside = red_edge(113, 138) + red_edge(186, 138);
    assert!(inside >= 390 && outside < 42, "moving silhouette: inside {inside}/420, outside {outside}/420");
}

#[test]
#[ignore = "requires the recorded 1080p BGRA pairs"]
fn stage2_replay_recorded_pans() {
    let input = std::path::PathBuf::from(std::env::var_os("FLOWSCALE_FG_REPLAY_INPUT").unwrap());
    let output = std::path::PathBuf::from(std::env::var_os("FLOWSCALE_FG_REPLAY_OUTPUT").unwrap());
    let ctx = D3d11Context::new(0).expect("hardware GPU required");
    let (width, height) = (1920, 1080);
    let pipeline = ComputePipeline::new_with_flow_scale(&ctx, width, height, 50).unwrap();
    let (interpolated, _, uav) = ctx.create_rw_texture(width, height, DXGI_FORMAT_B8G8R8A8_UNORM).unwrap();

    let load = |name: &str| {
        let bytes = std::fs::read(input.join(name)).unwrap();
        assert_eq!(bytes.len(), (width * height * 4) as usize);
        bytes.chunks_exact(4)
            .map(|bgra| u32::from_le_bytes(bgra.try_into().unwrap()))
            .collect::<Vec<_>>()
    };

    for (name, prev, curr) in [
        ("cs2", "cs2_02.bgra", "cs2_03.bgra"),
        ("roblox", "rblx_03.bgra", "rblx_05.bgra"),
    ] {
        let prev_pixels = load(prev);
        let curr_pixels = load(curr);
        let prev = upload(&ctx, width, height, &prev_pixels);
        let curr = upload(&ctx, width, height, &curr_pixels);
        pipeline.dispatch_motion_est(&ctx, &prev, &curr, width, height, 8).unwrap();
        pipeline.dispatch_frame_interp_ex(&ctx, &prev, &curr, &uav, width, height, 0.5, 0.5).unwrap();
        let pixels: Vec<[u8; 4]> = read(&ctx, &interpolated);
        if name == "cs2" {
            // This proxy detects transport of the gold HUD tint in this specific recording.
            let hud_tint = |[b, g, r, _]: [u8; 4]| {
                let [b, g, r] = [b, g, r].map(i16::from);
                r > 155 && g > 125 && r - b > 50 && g - b > 30 && r - g < 75
            };
            let bounds_x = 1670..1920;
            let bounds_y = 755..1010;
            let index = |x: i32, y: i32| (y as usize * width as usize) + x as usize;
            let in_sources = |x: i32, y: i32| {
                bounds_x.contains(&x) && bounds_y.contains(&y)
                    && (hud_tint(prev_pixels[index(x, y)].to_le_bytes())
                        || hud_tint(curr_pixels[index(x, y)].to_le_bytes()))
            };
            let mut shared = 0;
            let mut missing = 0;
            let mut spill = 0;
            for y in bounds_y.clone() {
                for x in bounds_x.clone() {
                    let i = index(x, y);
                    let generated = hud_tint(pixels[i]);
                    if hud_tint(prev_pixels[i].to_le_bytes()) && hud_tint(curr_pixels[i].to_le_bytes()) {
                        shared += 1;
                        missing += usize::from(!generated);
                    }
                    if generated && !(-2i32..=2).any(|dy| {
                        let span = 2 - dy.abs();
                        (-span..=span).any(|dx| in_sources(x + dx, y + dy))
                    }) {
                        spill += 1;
                    }
                }
            }
            eprintln!("CS2 stable HUD tint: {shared} shared, {missing} missing, {spill} spill");
            assert_eq!(shared, 3751, "unexpected CS2 replay input");
            assert!(missing < 150 && spill < 300, "CS2 HUD tint was displaced");
        }
        std::fs::write(output.join(format!("{name}_midpoint.bgra")), pixels.as_flattened()).unwrap();
    }
}
