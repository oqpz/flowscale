//! Clean-room D3D11 compute context, FLIP_DISCARD overlay swapchain, D2D1/DirectWrite
//! FPS counter, and original `cs_5_0` spatial-upscaling & optical-flow frame-generation shaders.

use std::ffi::c_void;
use std::fmt;
use std::mem::size_of;
use std::sync::Mutex;

use flowscale_abi::{FrameGenerationSize, ScalingFitMode, ScalingMode, ScalingType, SyncMode};
use flowscale_config::FpsCounterConfig;
use flowscale_overlay::CursorSnapshot;
use windows::core::{w, Interface, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, DXGI_STATUS_OCCLUDED, HANDLE, HMODULE, HWND, WAIT_OBJECT_0};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Factory, ID2D1RenderTarget, ID2D1SolidColorBrush,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_DEFAULT,
};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
    D3D11_SRV_DIMENSION_TEXTURE2D,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Buffer, ID3D11ComputeShader, ID3D11Device, ID3D11DeviceContext,
    ID3D11Multithread, ID3D11Query, ID3D11SamplerState, ID3D11ShaderResourceView,
    ID3D11Texture2D, ID3D11UnorderedAccessView, D3D11_ASYNC_GETDATA_DONOTFLUSH,
    D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE,
    D3D11_BIND_UNORDERED_ACCESS,
    D3D11_BUFFER_DESC, D3D11_COMPARISON_NEVER, D3D11_CPU_ACCESS_WRITE,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_FEATURE_D3D11_OPTIONS2,
    D3D11_FEATURE_DATA_D3D11_OPTIONS2, D3D11_FILTER_MIN_MAG_MIP_LINEAR, D3D11_MAP_WRITE_DISCARD,
    D3D11_MAPPED_SUBRESOURCE, D3D11_QUERY_DATA_TIMESTAMP_DISJOINT, D3D11_QUERY_DESC,
    D3D11_QUERY_TIMESTAMP, D3D11_QUERY_TIMESTAMP_DISJOINT, D3D11_SAMPLER_DESC, D3D11_SDK_VERSION,
    D3D11_SHADER_RESOURCE_VIEW_DESC, D3D11_SHADER_RESOURCE_VIEW_DESC_0, D3D11_TEX2D_SRV,
    D3D11_TEX2D_UAV, D3D11_TEXTURE2D_DESC, D3D11_TEXTURE_ADDRESS_CLAMP,
    D3D11_UAV_DIMENSION_TEXTURE2D, D3D11_UNORDERED_ACCESS_VIEW_DESC,
    D3D11_UNORDERED_ACCESS_VIEW_DESC_0, D3D11_USAGE_DEFAULT, D3D11_USAGE_DYNAMIC,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, DWRITE_FACTORY_TYPE_SHARED,
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_BOLD,
    DWRITE_MEASURING_MODE_NATURAL,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_COLOR_SPACE_RGB_FULL_G10_NONE_P709, DXGI_FORMAT,
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_FORMAT_R32G32_FLOAT, DXGI_FORMAT_R32G32B32A32_FLOAT,
    DXGI_FORMAT_R32_FLOAT, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter1, IDXGIAdapter3, IDXGIDevice3, IDXGIFactory1,
    IDXGIFactory2, IDXGIFactory5, IDXGIFactory6, IDXGISurface, IDXGISwapChain1, IDXGISwapChain2,
    IDXGISwapChain3, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_ERROR_DEVICE_REMOVED,
    DXGI_ERROR_DEVICE_RESET, DXGI_FEATURE_PRESENT_ALLOW_TEARING,
    DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, DXGI_MWA_NO_ALT_ENTER,
    DXGI_PRESENT, DXGI_PRESENT_ALLOW_TEARING, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING,
    DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT, DXGI_SWAP_EFFECT_FLIP_DISCARD,
    DXGI_USAGE_RENDER_TARGET_OUTPUT, DXGI_USAGE_UNORDERED_ACCESS,
};
use windows::Win32::System::Threading::WaitForSingleObjectEx;

pub const NEAREST_DXBC: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/nearest.cso"));
pub const BILINEAR_SHARP_DXBC: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/bilinear_sharp.cso"));
pub const BICUBIC_CAS_DXBC: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/bicubic_cas.cso"));
pub const SPATIAL_EASU_DXBC: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/spatial_easu.cso"));
pub const NIS_SHARPEN_DXBC: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/nis_sharpen.cso"));
pub const LS1_DETAIL_DXBC: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ls1_detail.cso"));
pub const ANIME4K_RESTORE_DXBC: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/anime4k_restore.cso"));
pub const SGSR_MOBILE_DXBC: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/sgsr_mobile.cso"));
pub const XBR_PIXEL_DXBC: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/xbr_pixel.cso"));
const MOTION_EST_COARSE_DXBC: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/motion_est_coarse.cso"));
pub const MOTION_EST_DXBC: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/motion_est.cso"));
pub const MOTION_LUMA_DXBC: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/motion_luma.cso"));
pub const MOTION_SMOOTH_DXBC: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/motion_smooth.cso"));
pub const FRAME_INTERP_DXBC: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/frame_interp.cso"));
pub const SDR_TO_SCRGB_DXBC: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/sdr_to_scrgb.cso"));
pub const OVERLAY_BLEND_DXBC: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/overlay_blend.cso"));

/// Returns SDR white as a multiplier of scRGB's 80-nit reference, for the window's output.
pub fn sdr_white_scale_for_window(hwnd: HWND) -> Result<f32, String> {
    use windows::Win32::Devices::Display::*;
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFOEXW, MONITOR_DEFAULTTONEAREST};
    unsafe {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        if !GetMonitorInfoW(monitor, (&mut info as *mut MONITORINFOEXW).cast()).as_bool() { return Err("GetMonitorInfo failed".into()); }
        for _ in 0..3 {
            let (mut paths_len, mut modes_len) = (0,0);
            GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut paths_len, &mut modes_len).ok().map_err(|e| e.to_string())?;
            let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); paths_len as usize];
            let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); modes_len as usize];
            let status = QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut paths_len, paths.as_mut_ptr(), &mut modes_len, modes.as_mut_ptr(), None);
            if status == windows::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER { continue; }
            status.ok().map_err(|e| e.to_string())?;
            for path in &paths[..paths_len as usize] {
                let mut name = DISPLAYCONFIG_SOURCE_DEVICE_NAME { header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                    r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, size: size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                    adapterId: path.sourceInfo.adapterId, id: path.sourceInfo.id }, ..Default::default() };
                if DisplayConfigGetDeviceInfo(&mut name.header) != 0 || name.viewGdiDeviceName != info.szDevice { continue; }
                let mut white = DISPLAYCONFIG_SDR_WHITE_LEVEL { header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                    r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL, size: size_of::<DISPLAYCONFIG_SDR_WHITE_LEVEL>() as u32,
                    adapterId: path.targetInfo.adapterId, id: path.targetInfo.id }, ..Default::default() };
                let code = DisplayConfigGetDeviceInfo(&mut white.header);
                if code != 0 { return Err(format!("GetSDRWhiteLevel Win32 {code}")); }
                if white.SDRWhiteLevel == 0 { return Err("SDR white level is zero".into()); }
                return Ok(white.SDRWhiteLevel as f32 / 1000.0);
            }
            return Err("monitor has no active display-config path".into());
        }
        Err("display topology changed repeatedly".into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GpuError {
    Dxgi(String),
    DeviceLost(String),
    D3d11(String),
    D2d1(String),
    DWrite(String),
}

impl fmt::Display for GpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Dxgi(s) => write!(f, "DXGI error: {s}"),
            Self::DeviceLost(s) => write!(f, "D3D11 device lost: {s}"),
            Self::D3d11(s) => write!(f, "D3D11 error: {s}"),
            Self::D2d1(s) => write!(f, "Direct2D error: {s}"),
            Self::DWrite(s) => write!(f, "DirectWrite error: {s}"),
        }
    }
}

impl std::error::Error for GpuError {}

/// Probed D3D11 device capabilities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct D3d11Capabilities {
    pub feature_level: u32,
    pub typed_uav_load_additional_formats: bool,
    pub adapter_name: String,
}

/// D3D11 device + immediate context + capability probe.
pub struct D3d11Context {
    pub factory: IDXGIFactory2,
    pub adapter: IDXGIAdapter1,
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    pub caps: D3d11Capabilities,
}

impl D3d11Context {
    /// Creates a D3D11 compute/render device on adapter `gpu_id` (falling back to 0 if needed).
    pub fn new(gpu_id: i32) -> Result<Self, GpuError> {
        let factory1: IDXGIFactory1 =
            unsafe { CreateDXGIFactory1().map_err(|e| GpuError::Dxgi(e.to_string()))? };
        let factory: IDXGIFactory2 = factory1
            .cast()
            .map_err(|e| GpuError::Dxgi(e.to_string()))?;

        let adapter = pick_hardware_adapter(&factory1, gpu_id)?;

        let desc = unsafe {
            adapter
                .GetDesc1()
                .map_err(|e| GpuError::Dxgi(e.to_string()))?
        };
        let end = desc
            .Description
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(desc.Description.len());
        let adapter_name = String::from_utf16_lossy(&desc.Description[..end]);

        let levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
        let mut feature_level = D3D_FEATURE_LEVEL::default();
        let mut device = None;
        let mut context = None;

        unsafe {
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                Some(&mut feature_level),
                Some(&mut context),
            )
            .map_err(|e| GpuError::D3d11(e.to_string()))?;
        }

        let device = device.ok_or_else(|| GpuError::D3d11("device is None".into()))?;
        let context = context.ok_or_else(|| GpuError::D3d11("context is None".into()))?;

        // Capture and rendering submit to this immediate context from separate threads.
        let mt: ID3D11Multithread = context
            .cast()
            .map_err(|e| GpuError::D3d11(e.to_string()))?;
        unsafe {
            let _ = mt.SetMultithreadProtected(true);
        }
        if let Ok(dxgi3) = device.cast::<IDXGIDevice3>() {
            unsafe {
                let _ = dxgi3.SetGPUThreadPriority(7);
            }
        }
        if let Ok(adapter3) = adapter.cast::<IDXGIAdapter3>() {
            let reserve = (desc.DedicatedVideoMemory as u64).min(256 * 1024 * 1024);
            unsafe {
                let _ = adapter3.SetVideoMemoryReservation(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, reserve);
            }
        }

        let mut opts = D3D11_FEATURE_DATA_D3D11_OPTIONS2::default();
        let _ = unsafe {
            device.CheckFeatureSupport(
                D3D11_FEATURE_D3D11_OPTIONS2,
                (&mut opts as *mut D3D11_FEATURE_DATA_D3D11_OPTIONS2).cast::<c_void>(),
                size_of::<D3D11_FEATURE_DATA_D3D11_OPTIONS2>() as u32,
            )
        };

        Ok(Self {
            factory,
            adapter,
            device,
            context,
            caps: D3d11Capabilities {
                feature_level: feature_level.0 as u32,
                typed_uav_load_additional_formats: opts.TypedUAVLoadAdditionalFormats.as_bool(),
                adapter_name,
            },
        })
    }

    /// Creates a compute shader from compiled DXBC bytecode.
    pub fn create_compute_shader(&self, dxbc: &[u8]) -> Result<ID3D11ComputeShader, GpuError> {
        let mut shader = None;
        unsafe {
            self.device
                .CreateComputeShader(dxbc, None, Some(&mut shader))
                .map_err(|e| GpuError::D3d11(e.to_string()))?;
        }
        shader.ok_or_else(|| GpuError::D3d11("CreateComputeShader returned None".into()))
    }

    /// Creates a Texture2D with both SRV and UAV bindings.
    pub fn create_rw_texture(
        &self,
        width: u32,
        height: u32,
        format: DXGI_FORMAT,
    ) -> Result<(ID3D11Texture2D, ID3D11ShaderResourceView, ID3D11UnorderedAccessView), GpuError>
    {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width.max(1),
            Height: height.max(1),
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_UNORDERED_ACCESS.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut tex = None;
        unsafe {
            self.device
                .CreateTexture2D(&desc, None, Some(&mut tex))
                .map_err(|e| GpuError::D3d11(e.to_string()))?;
        }
        let tex = tex.ok_or_else(|| GpuError::D3d11("CreateTexture2D returned None".into()))?;
        let srv = self.create_srv(&tex, format)?;
        let uav = self.create_uav(&tex, format)?;
        Ok((tex, srv, uav))
    }

    pub fn create_srv(
        &self,
        tex: &ID3D11Texture2D,
        format: DXGI_FORMAT,
    ) -> Result<ID3D11ShaderResourceView, GpuError> {
        create_srv_on(&self.device, tex, format)
    }

    pub fn create_uav(
        &self,
        tex: &ID3D11Texture2D,
        format: DXGI_FORMAT,
    ) -> Result<ID3D11UnorderedAccessView, GpuError> {
        create_uav_on(&self.device, tex, format)
    }

    /// Creates a dynamic 16-byte-aligned constant buffer.
    pub fn create_constant_buffer<T>(&self) -> Result<ID3D11Buffer, GpuError> {
        create_constant_buffer_on::<T>(&self.device)
    }

    /// Uploads `data` to a dynamic constant buffer using `MAP_WRITE_DISCARD`.
    pub fn update_constant_buffer<T: Copy>(
        &self,
        buf: &ID3D11Buffer,
        data: &T,
    ) -> Result<(), GpuError> {
        update_constant_buffer_on(&self.context, buf, data)
    }

    /// Creates a linear-clamp sampler for compute shaders.
    pub fn create_linear_clamp_sampler(&self) -> Result<ID3D11SamplerState, GpuError> {
        create_linear_clamp_sampler_on(&self.device)
    }
}

fn create_srv_on(
    device: &ID3D11Device,
    tex: &ID3D11Texture2D,
    format: DXGI_FORMAT,
) -> Result<ID3D11ShaderResourceView, GpuError> {
    let desc = D3D11_SHADER_RESOURCE_VIEW_DESC {
        Format: format,
        ViewDimension: D3D11_SRV_DIMENSION_TEXTURE2D,
        Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
            Texture2D: D3D11_TEX2D_SRV {
                MostDetailedMip: 0,
                MipLevels: 1,
            },
        },
    };
    let mut srv = None;
    unsafe {
        device
            .CreateShaderResourceView(tex, Some(&desc), Some(&mut srv))
            .map_err(|e| GpuError::D3d11(e.to_string()))?;
    }
    srv.ok_or_else(|| GpuError::D3d11("CreateShaderResourceView returned None".into()))
}

fn create_uav_on(
    device: &ID3D11Device,
    tex: &ID3D11Texture2D,
    format: DXGI_FORMAT,
) -> Result<ID3D11UnorderedAccessView, GpuError> {
    let desc = D3D11_UNORDERED_ACCESS_VIEW_DESC {
        Format: format,
        ViewDimension: D3D11_UAV_DIMENSION_TEXTURE2D,
        Anonymous: D3D11_UNORDERED_ACCESS_VIEW_DESC_0 {
            Texture2D: D3D11_TEX2D_UAV { MipSlice: 0 },
        },
    };
    let mut uav = None;
    unsafe {
        device
            .CreateUnorderedAccessView(tex, Some(&desc), Some(&mut uav))
            .map_err(|e| GpuError::D3d11(e.to_string()))?;
    }
    uav.ok_or_else(|| GpuError::D3d11("CreateUnorderedAccessView returned None".into()))
}

fn create_constant_buffer_on<T>(device: &ID3D11Device) -> Result<ID3D11Buffer, GpuError> {
    let byte_width = (size_of::<T>().div_ceil(16) * 16).max(16) as u32;
    let desc = D3D11_BUFFER_DESC {
        ByteWidth: byte_width,
        Usage: D3D11_USAGE_DYNAMIC,
        BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
        CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
        MiscFlags: 0,
        StructureByteStride: 0,
    };
    let mut buf = None;
    unsafe {
        device
            .CreateBuffer(&desc, None, Some(&mut buf))
            .map_err(|e| GpuError::D3d11(e.to_string()))?;
    }
    buf.ok_or_else(|| GpuError::D3d11("CreateBuffer returned None".into()))
}

fn update_constant_buffer_on<T: Copy>(
    context: &ID3D11DeviceContext,
    buf: &ID3D11Buffer,
    data: &T,
) -> Result<(), GpuError> {
    let mut desc = D3D11_BUFFER_DESC::default();
    unsafe { buf.GetDesc(&mut desc) };
    if size_of::<T>() > desc.ByteWidth as usize {
        return Err(GpuError::D3d11("constant buffer is smaller than the uploaded data".into()));
    }
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    unsafe {
        context
            .Map(buf, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut mapped))
            .map_err(|e| GpuError::D3d11(e.to_string()))?;
        // Mapped GPU memory need not satisfy a caller's over-aligned Rust type.
        std::ptr::copy_nonoverlapping(
            (data as *const T).cast::<u8>(), mapped.pData.cast::<u8>(), size_of::<T>()
        );
        context.Unmap(buf, 0);
    }
    Ok(())
}

fn create_linear_clamp_sampler_on(device: &ID3D11Device) -> Result<ID3D11SamplerState, GpuError> {
    let desc = D3D11_SAMPLER_DESC {
        Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
        AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
        AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
        AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
        MipLODBias: 0.0,
        MaxAnisotropy: 1,
        ComparisonFunc: D3D11_COMPARISON_NEVER,
        BorderColor: [0.0; 4],
        MinLOD: 0.0,
        MaxLOD: f32::MAX,
    };
    let mut sampler = None;
    unsafe {
        device
            .CreateSamplerState(&desc, Some(&mut sampler))
            .map_err(|e| GpuError::D3d11(e.to_string()))?;
    }
    sampler.ok_or_else(|| GpuError::D3d11("CreateSamplerState returned None".into()))
}

/// `DXGI_SWAP_EFFECT_FLIP_DISCARD` overlay swapchain with tearing & frame-latency control.
pub struct OverlaySwapChain {
    pub swap_chain: IDXGISwapChain2,
    pub waitable_object: HANDLE,
    pub tearing_supported: bool,
    pub format: DXGI_FORMAT,
    cached_uavs: Mutex<Vec<(usize, ID3D11Texture2D, ID3D11UnorderedAccessView)>>,
}

unsafe impl Send for OverlaySwapChain {}
unsafe impl Sync for OverlaySwapChain {}

impl Drop for OverlaySwapChain {
    fn drop(&mut self) {
        if !self.waitable_object.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.waitable_object);
            }
            self.waitable_object = HANDLE::default();
        }
    }
}

impl OverlaySwapChain {
    pub fn new(
        ctx: &D3d11Context,
        hwnd: HWND,
        width: u32,
        height: u32,
        max_frame_latency: u32,
        hdr_support: bool,
    ) -> Result<Self, GpuError> {
        let mut tearing_supported = false;
        if let Ok(factory5) = ctx.factory.cast::<IDXGIFactory5>() {
            let mut allow_tearing: i32 = 0;
            if unsafe {
                factory5.CheckFeatureSupport(
                    DXGI_FEATURE_PRESENT_ALLOW_TEARING,
                    (&mut allow_tearing as *mut i32).cast::<c_void>(),
                    size_of::<i32>() as u32,
                )
            }
            .is_ok()
            {
                tearing_supported = allow_tearing != 0;
            }
        }

        let format = if hdr_support {
            DXGI_FORMAT_R16G16B16A16_FLOAT
        } else {
            DXGI_FORMAT_B8G8R8A8_UNORM
        };

        let mut flags = DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 as u32;
        if tearing_supported {
            flags |= DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING.0 as u32;
        }

        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: width.max(1),
            Height: height.max(1),
            Format: format,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT | DXGI_USAGE_UNORDERED_ACCESS,
            BufferCount: 3,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            Flags: flags,
        };

        let sc1: IDXGISwapChain1 = unsafe {
            ctx.factory
                .CreateSwapChainForHwnd(&ctx.device, hwnd, &desc, None, None)
                .map_err(|e| GpuError::Dxgi(e.to_string()))?
        };
        unsafe {
            let _ = ctx.factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER);
        }
        let swap_chain: IDXGISwapChain2 = sc1
            .cast()
            .map_err(|e| GpuError::Dxgi(e.to_string()))?;
        if hdr_support {
            let sc3 = swap_chain.cast::<IDXGISwapChain3>().map_err(Self::swap_chain_error)?;
            unsafe {
                let support = sc3.CheckColorSpaceSupport(DXGI_COLOR_SPACE_RGB_FULL_G10_NONE_P709).map_err(Self::swap_chain_error)?;
                if support & windows::Win32::Graphics::Dxgi::DXGI_SWAP_CHAIN_COLOR_SPACE_SUPPORT_FLAG_PRESENT.0 as u32 == 0 {
                    return Err(GpuError::Dxgi("linear scRGB presentation is unsupported on this output".into()));
                }
                sc3.SetColorSpace1(DXGI_COLOR_SPACE_RGB_FULL_G10_NONE_P709).map_err(Self::swap_chain_error)?;
            }
        }
        unsafe {
            swap_chain
                .SetMaximumFrameLatency(max_frame_latency.clamp(1, 3))
                .map_err(|e| GpuError::Dxgi(e.to_string()))?;
        }
        let waitable_object = unsafe { swap_chain.GetFrameLatencyWaitableObject() };

        Ok(Self {
            swap_chain,
            waitable_object,
            tearing_supported,
            format,
            cached_uavs: Mutex::new(Vec::new()),
        })
    }

    /// Waits up to `timeout_ms` on DXGI's frame-latency waitable object so CPU pacing stays aligned
    /// with DWM flip completion without blocking GPU command submission.
    ///
    /// Reports readiness, timeout, or the Win32 failure code separately.
    pub fn wait_for_frame_latency(&self, timeout_ms: u32) -> FrameLatencyWait {
        if self.waitable_object.is_invalid() {
            return FrameLatencyWait::Signaled;
        }
        let res = unsafe { WaitForSingleObjectEx(self.waitable_object, timeout_ms, false) };
        if res == WAIT_OBJECT_0 { FrameLatencyWait::Signaled }
        else if res == windows::Win32::Foundation::WAIT_TIMEOUT { FrameLatencyWait::Timeout }
        else { FrameLatencyWait::Failed(unsafe { windows::Win32::Foundation::GetLastError() }.0) }
    }

    /// Updates DXGI's maximum frame latency (`1..=3` for the 3-buffer flip-discard chain).
    pub fn set_max_frame_latency(&self, max_frame_latency: u32) -> Result<(), GpuError> {
        unsafe {
            self.swap_chain
                .SetMaximumFrameLatency(max_frame_latency.clamp(1, 3))
                .map_err(Self::swap_chain_error)
        }
    }

    /// Returns whether `DXGI_FEATURE_PRESENT_ALLOW_TEARING` is active on this swapchain.
    pub fn tearing_supported(&self) -> bool {
        self.tearing_supported
    }

    /// Computes `(sync_interval, present_flags)` for `IDXGISwapChain::Present`.
    pub fn present_params(sync_mode: SyncMode, tearing_supported: bool) -> (u32, DXGI_PRESENT) {
        match sync_mode {
            SyncMode::Off => {
                let flags = if tearing_supported {
                    DXGI_PRESENT_ALLOW_TEARING
                } else {
                    DXGI_PRESENT(0)
                };
                (0, flags)
            }
            SyncMode::Default => (0, DXGI_PRESENT(0)),
            SyncMode::Vsync1 => (1, DXGI_PRESENT(0)),
            SyncMode::Vsync2 => (2, DXGI_PRESENT(0)),
            SyncMode::Vsync3 => (3, DXGI_PRESENT(0)),
            SyncMode::Vsync4 => (4, DXGI_PRESENT(0)),
        }
    }

    pub fn back_buffer(&self) -> Result<ID3D11Texture2D, GpuError> {
        unsafe {
            self.swap_chain
                .GetBuffer::<ID3D11Texture2D>(0)
                .map_err(|e| GpuError::Dxgi(e.to_string()))
        }
    }

    /// Returns a cached unordered-access view of the current swapchain back buffer for compute output.
    pub fn back_buffer_uav(&self, ctx: &D3d11Context) -> Result<ID3D11UnorderedAccessView, GpuError> {
        let bb = self.back_buffer()?;
        let tex_ptr = bb.as_raw() as usize;
        let mut cache = self.cached_uavs.lock().unwrap();
        if let Some((_, _, uav)) = cache.iter().find(|(p, _, _)| *p == tex_ptr) {
            return Ok(uav.clone());
        }
        let uav = ctx.create_uav(&bb, self.format)?;
        if cache.len() >= 4 {
            cache.clear();
        }
        cache.push((tex_ptr, bb, uav.clone()));
        Ok(uav)
    }

    pub fn resize(&self, width: u32, height: u32) -> Result<(), GpuError> {
        // Release all held backbuffer/UAV references before DXGI ResizeBuffers.
        self.cached_uavs.lock().unwrap().clear();
        let mut flags = DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0;
        if self.tearing_supported {
            flags |= DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING.0;
        }
        unsafe {
            self.swap_chain
                .ResizeBuffers(
                    0,
                    width.max(1),
                    height.max(1),
                    DXGI_FORMAT_UNKNOWN,
                    DXGI_SWAP_CHAIN_FLAG(flags),
                )
                .map_err(Self::swap_chain_error)
        }
    }

    fn swap_chain_error(error: windows::core::Error) -> GpuError {
        match error.code() {
            DXGI_ERROR_DEVICE_REMOVED | DXGI_ERROR_DEVICE_RESET => {
                GpuError::DeviceLost(error.to_string())
            }
            _ => GpuError::Dxgi(error.to_string()),
        }
    }

    pub fn present(&self, sync_mode: SyncMode) -> Result<(), GpuError> {
        self.present_outcome(sync_mode).map(|_| ())
    }

    pub fn present_outcome(&self, sync_mode: SyncMode) -> Result<PresentOutcome, GpuError> {
        let (interval, flags) = Self::present_params(sync_mode, self.tearing_supported);
        let hr = unsafe { self.swap_chain.Present(interval, flags) };
        if hr == DXGI_STATUS_OCCLUDED {
            return Ok(PresentOutcome::Occluded);
        }
        hr.ok().map(|()| PresentOutcome::Presented).map_err(Self::swap_chain_error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameLatencyWait { Signaled, Timeout, Failed(u32) }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentOutcome { Presented, Occluded }

#[repr(C)]
#[derive(Clone, Copy)]
struct OverlayBlendParams {
    dst_x: u32,
    dst_y: u32,
    dst_w: u32,
    dst_h: u32,
    src_w: u32,
    src_h: u32,
    src_scale_x: f32,
    src_scale_y: f32,
    src_off_x: f32,
    src_off_y: f32,
    hdr_white_scale: f32,
    _pad: u32,
}

struct CachedFpsTexture {
    width: u32,
    height: u32,
    _tex: ID3D11Texture2D,
    srv: ID3D11ShaderResourceView,
    rt: ID2D1RenderTarget,
    brush: ID2D1SolidColorBrush,
    last_fps: Option<(u32, u32)>,
}

struct CachedCursorTexture {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    tex: ID3D11Texture2D,
    srv: ID3D11ShaderResourceView,
}

struct FpsGpuResources {
    device_ptr: usize,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    cs_blend: ID3D11ComputeShader,
    blend_cb: ID3D11Buffer,
    linear_clamp: ID3D11SamplerState,
    fps_cache: Option<CachedFpsTexture>,
    cursor_cache: Option<CachedCursorTexture>,
}

#[derive(Default)]
struct FpsOverlayScratch {
    cursor_bgra: Vec<u8>,
    fps_utf16: Vec<u16>,
    gpu: Option<FpsGpuResources>,
}

fn push_u32_utf16(out: &mut Vec<u16>, mut value: u32) {
    let mut digits = [0u16; 10];
    let mut len = 0usize;
    loop {
        digits[len] = b'0' as u16 + (value % 10) as u16;
        len += 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    for &d in digits[..len].iter().rev() {
        out.push(d);
    }
}

fn clamp_overlay_rect(
    left: f32,
    top: f32,
    draw_w: f32,
    draw_h: f32,
    bb_w: u32,
    bb_h: u32,
) -> Option<(u32, u32, u32, u32, f32, f32)> {
    let right = left + draw_w;
    let bottom = top + draw_h;
    let max_w = bb_w as f32;
    let max_h = bb_h as f32;
    let x0 = left.floor().clamp(0.0, max_w) as u32;
    let y0 = top.floor().clamp(0.0, max_h) as u32;
    let x1 = right.ceil().clamp(0.0, max_w) as u32;
    let y1 = bottom.ceil().clamp(0.0, max_h) as u32;
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some((x0, y0, x1 - x0, y1 - y0, left - x0 as f32, top - y0 as f32))
}

fn dispatch_overlay_blend(
    gpu: &FpsGpuResources,
    overlay_srv: &ID3D11ShaderResourceView,
    bb_uav: &ID3D11UnorderedAccessView,
    params: &OverlayBlendParams,
) -> Result<(), GpuError> {
    update_constant_buffer_on(&gpu.context, &gpu.blend_cb, params)?;
    unsafe {
        gpu.context.CSSetShader(&gpu.cs_blend, None);
        gpu.context
            .CSSetConstantBuffers(0, Some(&[Some(gpu.blend_cb.clone())]));
        gpu.context
            .CSSetShaderResources(0, Some(&[Some(overlay_srv.clone())]));
        gpu.context
            .CSSetUnorderedAccessViews(0, 1, Some(&Some(bb_uav.clone())), None);
        gpu.context
            .CSSetSamplers(0, Some(&[Some(gpu.linear_clamp.clone())]));
        gpu.context
            .Dispatch(params.dst_w.div_ceil(8), params.dst_h.div_ceil(8), 1);
        gpu.context.CSSetShaderResources(0, Some(&[None]));
        gpu.context
            .CSSetUnorderedAccessViews(0, 1, Some(&None), None);
    }
    Ok(())
}

/// Direct2D + DirectWrite FPS counter and software cursor overlay composited via compute shader.
pub struct FpsOverlay {
    d2d_factory: ID2D1Factory,
    dwrite_factory: IDWriteFactory,
    text_format: IDWriteTextFormat,
    font_size: f32,
    color_rgb: u32,
    show_captured: bool,
    scratch: Mutex<FpsOverlayScratch>,
}

impl FpsOverlay {
    pub fn new(font_size: f32) -> Result<Self, GpuError> {
        Self::from_config_with_size(&FpsCounterConfig::default(), font_size)
    }

    /// Creates an overlay configured from `[fps_counter]` (`font`, `color`, `scale`, `show_captured`).
    pub fn from_config(cfg: &FpsCounterConfig) -> Result<Self, GpuError> {
        Self::from_config_with_size(cfg, 18.0 * cfg.scale.clamp(0.2, 5.0))
    }

    fn from_config_with_size(cfg: &FpsCounterConfig, font_size: f32) -> Result<Self, GpuError> {
        let font_size = font_size.max(8.0);
        let d2d_factory: ID2D1Factory = unsafe {
            D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)
                .map_err(|e| GpuError::D2d1(e.to_string()))?
        };
        let dwrite_factory: IDWriteFactory = unsafe {
            DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)
                .map_err(|e| GpuError::DWrite(e.to_string()))?
        };
        let font_name = if cfg.font.trim().is_empty() {
            "Consolas"
        } else {
            cfg.font.trim()
        };
        let mut font_w: Vec<u16> = font_name.encode_utf16().collect();
        font_w.push(0);
        let text_format = unsafe {
            dwrite_factory
                .CreateTextFormat(
                    PCWSTR(font_w.as_ptr()),
                    None,
                    DWRITE_FONT_WEIGHT_BOLD,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    font_size,
                    w!("en-us"),
                )
                .map_err(|e| GpuError::DWrite(e.to_string()))?
        };
        Ok(Self {
            d2d_factory,
            dwrite_factory,
            text_format,
            font_size,
            color_rgb: cfg.color,
            show_captured: cfg.show_captured,
            scratch: Mutex::new(FpsOverlayScratch::default()),
        })
    }

    /// Draws `"<captured> / <presented>"` onto `back_buffer` using `DXGI_FORMAT_B8G8R8A8_UNORM`.
    pub fn draw(
        &self,
        back_buffer: &ID3D11Texture2D,
        captured_fps: u32,
        presented_fps: u32,
        offset_x: f32,
        offset_y: f32,
    ) -> Result<(), GpuError> {
        self.draw_for_format(
            back_buffer,
            captured_fps,
            presented_fps,
            offset_x,
            offset_y,
            DXGI_FORMAT_B8G8R8A8_UNORM,
        )
    }

    /// Draws the configured FPS string onto `back_buffer` matching the swapchain pixel format.
    pub fn draw_for_format(
        &self,
        back_buffer: &ID3D11Texture2D,
        captured_fps: u32,
        presented_fps: u32,
        offset_x: f32,
        offset_y: f32,
        format: DXGI_FORMAT,
    ) -> Result<(), GpuError> {
        self.draw_overlays(
            back_buffer,
            format,
            Some((captured_fps, presented_fps, offset_x, offset_y)),
            None,
        )
    }

    /// Composites a captured software cursor bitmap onto `back_buffer` at `(x, y)` scaled by `scale`.
    pub fn draw_cursor(
        &self,
        back_buffer: &ID3D11Texture2D,
        format: DXGI_FORMAT,
        cursor: &CursorSnapshot,
        x: f32,
        y: f32,
        scale: f32,
    ) -> Result<(), GpuError> {
        self.draw_overlays(back_buffer, format, None, Some((cursor, x, y, scale)))
    }

    /// Composites optional software cursor and/or FPS counter onto `back_buffer` using cached
    /// offscreen textures and the `overlay_blend` compute shader (zero swapchain RT churn).
    pub fn draw_overlays(
        &self,
        back_buffer: &ID3D11Texture2D,
        format: DXGI_FORMAT,
        fps: Option<(u32, u32, f32, f32)>,
        cursor: Option<(&CursorSnapshot, f32, f32, f32)>,
    ) -> Result<(), GpuError> {
        self.draw_overlays_with_white(back_buffer, format, fps, cursor, 1.0)
    }

    pub fn draw_overlays_with_white(&self, back_buffer: &ID3D11Texture2D, format: DXGI_FORMAT,
        fps: Option<(u32,u32,f32,f32)>, cursor: Option<(&CursorSnapshot,f32,f32,f32)>, white_scale: f32) -> Result<(), GpuError> {
        if fps.is_none() && cursor.is_none() {
            return Ok(());
        }

        let device: ID3D11Device = unsafe { back_buffer.GetDevice() }
            .map_err(|e| GpuError::D3d11(e.to_string()))?;
        let device_ptr = device.as_raw() as usize;

        let mut bb_desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { back_buffer.GetDesc(&mut bb_desc) };
        let uav_format = if format == DXGI_FORMAT_UNKNOWN {
            bb_desc.Format
        } else {
            format
        };
        let bb_uav = create_uav_on(&device, back_buffer, uav_format)?;
        let hdr_white_scale = if uav_format == DXGI_FORMAT_R16G16B16A16_FLOAT {
            white_scale
        } else {
            1.0
        };

        let mut scratch = self.scratch.lock().unwrap();
        let needs_init = scratch
            .gpu
            .as_ref()
            .is_none_or(|g| g.device_ptr != device_ptr);
        if needs_init {
            let context = unsafe { device.GetImmediateContext() }
                .map_err(|e| GpuError::D3d11(e.to_string()))?;
            let mut cs_blend = None;
            unsafe {
                device
                    .CreateComputeShader(OVERLAY_BLEND_DXBC, None, Some(&mut cs_blend))
                    .map_err(|e| GpuError::D3d11(e.to_string()))?;
            }
            let cs_blend =
                cs_blend.ok_or_else(|| GpuError::D3d11("CreateComputeShader returned None".into()))?;
            let blend_cb = create_constant_buffer_on::<OverlayBlendParams>(&device)?;
            let linear_clamp = create_linear_clamp_sampler_on(&device)?;
            scratch.gpu = Some(FpsGpuResources {
                device_ptr,
                device,
                context,
                cs_blend,
                blend_cb,
                linear_clamp,
                fps_cache: None,
                cursor_cache: None,
            });
        }

        let FpsOverlayScratch {
            cursor_bgra,
            fps_utf16,
            gpu,
        } = &mut *scratch;
        let gpu = gpu.as_mut().expect("gpu resources initialized");

        if let Some((snap, cx, cy, raw_scale)) = cursor {
            let expected = (snap.width as usize).checked_mul(snap.height as usize).and_then(|n| n.checked_mul(4));
            if snap.visible
                && (1..=512).contains(&snap.width)
                && (1..=512).contains(&snap.height)
                && expected.is_some_and(|n| snap.rgba.len() >= n)
                && raw_scale.is_finite()
                && raw_scale > 0.0
            {
                let expected = expected.unwrap();
                let cursor_unchanged = gpu.cursor_cache.as_ref().is_some_and(|c| {
                    c.width == snap.width
                        && c.height == snap.height
                        && c.rgba.as_slice() == &snap.rgba[..expected]
                });
                if !cursor_unchanged {
                    cursor_bgra.clear();
                    cursor_bgra.reserve(expected);
                    for px in snap.rgba[..expected].chunks_exact(4) {
                        let a = u16::from(px[3]);
                        let r = ((u16::from(px[0]) * a + 127) / 255) as u8;
                        let g = ((u16::from(px[1]) * a + 127) / 255) as u8;
                        let b = ((u16::from(px[2]) * a + 127) / 255) as u8;
                        cursor_bgra.extend_from_slice(&[b, g, r, px[3]]);
                    }
                    let same_dims = gpu
                        .cursor_cache
                        .as_ref()
                        .is_some_and(|c| c.width == snap.width && c.height == snap.height);
                    if same_dims {
                        let cache = gpu.cursor_cache.as_mut().unwrap();
                        unsafe {
                            gpu.context.UpdateSubresource(
                                &cache.tex,
                                0,
                                None,
                                cursor_bgra.as_ptr().cast(),
                                snap.width * 4,
                                0,
                            );
                        }
                        cache.rgba.clear();
                        cache.rgba.extend_from_slice(&snap.rgba[..expected]);
                    } else {
                        let desc = D3D11_TEXTURE2D_DESC {
                            Width: snap.width,
                            Height: snap.height,
                            MipLevels: 1,
                            ArraySize: 1,
                            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                            SampleDesc: DXGI_SAMPLE_DESC {
                                Count: 1,
                                Quality: 0,
                            },
                            Usage: D3D11_USAGE_DEFAULT,
                            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                            CPUAccessFlags: 0,
                            MiscFlags: 0,
                        };
                        let mut tex = None;
                        unsafe {
                            gpu.device
                                .CreateTexture2D(&desc, None, Some(&mut tex))
                                .map_err(|e| GpuError::D3d11(e.to_string()))?;
                        }
                        let tex = tex.ok_or_else(|| {
                            GpuError::D3d11("CreateTexture2D returned None".into())
                        })?;
                        unsafe {
                            gpu.context.UpdateSubresource(
                                &tex,
                                0,
                                None,
                                cursor_bgra.as_ptr().cast(),
                                snap.width * 4,
                                0,
                            );
                        }
                        let srv = create_srv_on(&gpu.device, &tex, DXGI_FORMAT_B8G8R8A8_UNORM)?;
                        gpu.cursor_cache = Some(CachedCursorTexture {
                            width: snap.width,
                            height: snap.height,
                            rgba: snap.rgba[..expected].to_vec(),
                            tex,
                            srv,
                        });
                    }
                }
                if let Some(cache) = gpu.cursor_cache.as_ref() {
                    let s = raw_scale;
                    let left = cx - (snap.hotspot_x as f32) * s;
                    let top = cy - (snap.hotspot_y as f32) * s;
                    if let Some((dst_x, dst_y, dst_w, dst_h, src_off_x, src_off_y)) =
                        clamp_overlay_rect(
                            left,
                            top,
                            (snap.width as f32) * s,
                            (snap.height as f32) * s,
                            bb_desc.Width,
                            bb_desc.Height,
                        )
                    {
                        let params = OverlayBlendParams {
                            dst_x,
                            dst_y,
                            dst_w,
                            dst_h,
                            src_w: snap.width,
                            src_h: snap.height,
                            src_scale_x: s,
                            src_scale_y: s,
                            src_off_x,
                            src_off_y,
                            hdr_white_scale,
                            _pad: u32::from(uav_format == DXGI_FORMAT_R16G16B16A16_FLOAT),
                        };
                        dispatch_overlay_blend(gpu, &cache.srv, &bb_uav, &params)?;
                    }
                }
            }
        }

        if let Some((cap, pres, ox, oy)) = fps {
            if gpu.fps_cache.is_none() {
                let fps_w = (self.font_size * 8.0).ceil().max(320.0) as u32;
                let fps_h = (self.font_size * 1.6).ceil().max(80.0) as u32;
                let desc = D3D11_TEXTURE2D_DESC {
                    Width: fps_w,
                    Height: fps_h,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
                    CPUAccessFlags: 0,
                    MiscFlags: 0,
                };
                let mut tex = None;
                unsafe {
                    gpu.device
                        .CreateTexture2D(&desc, None, Some(&mut tex))
                        .map_err(|e| GpuError::D3d11(e.to_string()))?;
                }
                let tex =
                    tex.ok_or_else(|| GpuError::D3d11("CreateTexture2D returned None".into()))?;
                let srv = create_srv_on(&gpu.device, &tex, DXGI_FORMAT_B8G8R8A8_UNORM)?;
                let surface: IDXGISurface = tex
                    .cast()
                    .map_err(|e| GpuError::Dxgi(e.to_string()))?;
                let props = D2D1_RENDER_TARGET_PROPERTIES {
                    r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                    pixelFormat: D2D1_PIXEL_FORMAT {
                        format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                    },
                    dpiX: 96.0,
                    dpiY: 96.0,
                    ..Default::default()
                };
                let rt: ID2D1RenderTarget = unsafe {
                    self.d2d_factory
                        .CreateDxgiSurfaceRenderTarget(&surface, &props)
                        .map_err(|e| GpuError::D2d1(e.to_string()))?
                };
                let r = ((self.color_rgb >> 16) & 0xFF) as f32 / 255.0;
                let g = ((self.color_rgb >> 8) & 0xFF) as f32 / 255.0;
                let b = (self.color_rgb & 0xFF) as f32 / 255.0;
                let brush: ID2D1SolidColorBrush = unsafe {
                    rt.CreateSolidColorBrush(&D2D1_COLOR_F { r, g, b, a: 1.0 }, None)
                        .map_err(|e| GpuError::D2d1(e.to_string()))?
                };
                gpu.fps_cache = Some(CachedFpsTexture {
                    width: fps_w,
                    height: fps_h,
                    _tex: tex,
                    srv,
                    rt,
                    brush,
                    last_fps: None,
                });
            }

            let key = (if self.show_captured { cap } else { 0 }, pres);
            let end_res = {
                let cache = gpu.fps_cache.as_mut().unwrap();
                if cache.last_fps != Some(key) {
                    fps_utf16.clear();
                    if self.show_captured {
                        push_u32_utf16(fps_utf16, cap);
                        fps_utf16.extend_from_slice(&[b' ' as u16, b'/' as u16, b' ' as u16]);
                    }
                    push_u32_utf16(fps_utf16, pres);
                    let local_rect = D2D_RECT_F {
                        left: 0.0,
                        top: 0.0,
                        right: cache.width as f32,
                        bottom: cache.height as f32,
                    };
                    let _ = &self.dwrite_factory;
                    let res = unsafe {
                        cache.rt.BeginDraw();
                        cache.rt.Clear(Some(&D2D1_COLOR_F {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 0.0,
                        }));
                        cache.rt.DrawText(
                            fps_utf16,
                            &self.text_format,
                            &local_rect,
                            &cache.brush,
                            Default::default(),
                            DWRITE_MEASURING_MODE_NATURAL,
                        );
                        cache.rt.EndDraw(None, None)
                    };
                    if res.is_ok() {
                        cache.last_fps = Some(key);
                    }
                    res
                } else {
                    Ok(())
                }
            };
            if let Err(e) = end_res {
                gpu.fps_cache = None;
                return Err(GpuError::D2d1(e.to_string()));
            }

            if let Some(cache) = gpu.fps_cache.as_ref() {
                if let Some((dst_x, dst_y, dst_w, dst_h, src_off_x, src_off_y)) = clamp_overlay_rect(
                    ox,
                    oy,
                    cache.width as f32,
                    cache.height as f32,
                    bb_desc.Width,
                    bb_desc.Height,
                ) {
                    let params = OverlayBlendParams {
                        dst_x,
                        dst_y,
                        dst_w,
                        dst_h,
                        src_w: cache.width,
                        src_h: cache.height,
                        src_scale_x: 1.0,
                        src_scale_y: 1.0,
                        src_off_x,
                        src_off_y,
                        hdr_white_scale,
                        _pad: u32::from(uav_format == DXGI_FORMAT_R16G16B16A16_FLOAT),
                    };
                    dispatch_overlay_blend(gpu, &cache.srv, &bb_uav, &params)?;
                }
            }
        }

        Ok(())
    }
}

/// Computes the target `[offset_x, offset_y, dst_w, dst_h]` rectangle inside `out_w x out_h`.
pub fn compute_dst_rect(
    in_w: u32,
    in_h: u32,
    out_w: u32,
    out_h: u32,
    scaling_mode: ScalingMode,
    fit_mode: ScalingFitMode,
    scale_factor: f32,
) -> [u32; 4] {
    let iw = in_w.max(1);
    let ih = in_h.max(1);
    let ow = out_w.max(1);
    let oh = out_h.max(1);

    match scaling_mode {
        ScalingMode::Auto => match fit_mode {
            ScalingFitMode::Fullscreen => [0, 0, ow, oh],
            ScalingFitMode::AspectRatio => {
                let scale = (f64::from(ow) / f64::from(iw)).min(f64::from(oh) / f64::from(ih));
                let dw = (f64::from(iw) * scale).round().clamp(1.0, f64::from(ow)) as u32;
                let dh = (f64::from(ih) * scale).round().clamp(1.0, f64::from(oh)) as u32;
                [(ow - dw) / 2, (oh - dh) / 2, dw, dh]
            }
        },
        ScalingMode::Custom => {
            let sf = if scale_factor.is_finite() {
                f64::from(scale_factor.clamp(1.0, 10.0))
            } else {
                1.0
            };
            let raw_w = (f64::from(iw) * sf).round().max(1.0);
            let raw_h = (f64::from(ih) * sf).round().max(1.0);
            if fit_mode == ScalingFitMode::AspectRatio
                && (raw_w > f64::from(ow) || raw_h > f64::from(oh))
            {
                let fit = (f64::from(ow) / raw_w).min(f64::from(oh) / raw_h);
                let dw = (raw_w * fit).round().clamp(1.0, f64::from(ow)) as u32;
                let dh = (raw_h * fit).round().clamp(1.0, f64::from(oh)) as u32;
                [(ow - dw) / 2, (oh - dh) / 2, dw, dh]
            } else {
                let dw = (raw_w as u32).clamp(1, ow);
                let dh = (raw_h as u32).clamp(1, oh);
                [(ow - dw) / 2, (oh - dh) / 2, dw, dh]
            }
        }
    }
}

/// Maps `FrameGenerationSize` to optical-flow block search radius (`Balanced=8`, `Performance=5`).
pub fn search_radius_for_fg_size(size: FrameGenerationSize) -> i32 {
    match size {
        FrameGenerationSize::Balanced => 8,
        FrameGenerationSize::Performance => 5,
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct UpscaleParams {
    input_size: [u32; 2],
    output_size: [u32; 2],
    dst_rect: [u32; 4],
    sharpness: f32,
    mode_flags: u32,
    subtype: u32,
    vrs_flag: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MotionParams {
    frame_size: [u32; 2],
    flow_size: [u32; 2],
    search_radius: i32,
    block_radius: u32,
    pass_mode: u32,
    coarse_stride: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct InterpParams {
    frame_size: [u32; 2],
    alpha: f32,
    ui_threshold: f32,
}

const GPU_TIMER_RING_LEN: usize = 3;

struct GpuTimerSlot {
    disjoint: ID3D11Query,
    motion_start: ID3D11Query,
    motion_end: ID3D11Query,
    interp_start: ID3D11Query,
    interp_end: ID3D11Query,
    upscale_start: ID3D11Query,
    upscale_end: ID3D11Query,
    active: bool,
    pending: bool,
    generation: u64,
    motion_recorded: bool,
    interp_recorded: bool,
    upscale_recorded: bool,
}

impl GpuTimerSlot {
    fn new(device: &ID3D11Device) -> Result<Self, GpuError> {
        let make_query = |query_type| -> Result<ID3D11Query, GpuError> {
            let desc = D3D11_QUERY_DESC {
                Query: query_type,
                MiscFlags: 0,
            };
            let mut q = None;
            unsafe {
                device
                    .CreateQuery(&desc, Some(&mut q))
                    .map_err(|e| GpuError::D3d11(e.to_string()))?;
            }
            q.ok_or_else(|| GpuError::D3d11("CreateQuery returned None".into()))
        };
        Ok(Self {
            disjoint: make_query(D3D11_QUERY_TIMESTAMP_DISJOINT)?,
            motion_start: make_query(D3D11_QUERY_TIMESTAMP)?,
            motion_end: make_query(D3D11_QUERY_TIMESTAMP)?,
            interp_start: make_query(D3D11_QUERY_TIMESTAMP)?,
            interp_end: make_query(D3D11_QUERY_TIMESTAMP)?,
            upscale_start: make_query(D3D11_QUERY_TIMESTAMP)?,
            upscale_end: make_query(D3D11_QUERY_TIMESTAMP)?,
            active: false,
            pending: false,
            generation: 0,
            motion_recorded: false,
            interp_recorded: false,
            upscale_recorded: false,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct GpuTimingSample {
    pub generation: u64,
    pub motion_ms: Option<f32>,
    pub interpolation_ms: Option<f32>,
    pub upscale_ms: Option<f32>,
}

struct GpuTimerRing {
    slots: [GpuTimerSlot; GPU_TIMER_RING_LEN],
    write_idx: usize,
    last_timings: Option<GpuTimingSample>,
    generation: u64,
}

impl GpuTimerRing {
    fn new(device: &ID3D11Device) -> Result<Self, GpuError> {
        Ok(Self {
            slots: [
                GpuTimerSlot::new(device)?,
                GpuTimerSlot::new(device)?,
                GpuTimerSlot::new(device)?,
            ],
            write_idx: 0,
            last_timings: None,
            generation: 0,
        })
    }

    fn ensure_active(&mut self, context: &ID3D11DeviceContext) -> bool {
        if self.slots[self.write_idx].active { return true; }
        let Some(idx) = (0..GPU_TIMER_RING_LEN).map(|offset| (self.write_idx+offset)%GPU_TIMER_RING_LEN).find(|idx| !self.slots[*idx].pending) else { return false; };
        self.write_idx = idx;
        self.generation += 1;
        let slot = &mut self.slots[idx];
        slot.motion_recorded = false; slot.interp_recorded = false; slot.upscale_recorded = false;
        slot.active = true; slot.generation = self.generation;
        unsafe { context.Begin(&slot.disjoint); }
        true
    }

    fn finish_frame_and_poll(&mut self, context: &ID3D11DeviceContext) {
        let slot = &mut self.slots[self.write_idx];
        if slot.active {
            unsafe {
                context.End(&slot.disjoint);
            }
            slot.active = false;
            slot.pending = true;
            self.write_idx = (self.write_idx + 1) % GPU_TIMER_RING_LEN;
        }

        // Poll oldest-to-newest pending slots non-blockingly with DONOTFLUSH.
        for offset in 0..GPU_TIMER_RING_LEN {
            let idx = (self.write_idx + offset) % GPU_TIMER_RING_LEN;
            let s = &mut self.slots[idx];
            if !s.pending {
                continue;
            }
            let mut disjoint_data = D3D11_QUERY_DATA_TIMESTAMP_DISJOINT::default();
            let res = unsafe {
                context.GetData(
                    &s.disjoint,
                    Some((&mut disjoint_data as *mut D3D11_QUERY_DATA_TIMESTAMP_DISJOINT).cast()),
                    size_of::<D3D11_QUERY_DATA_TIMESTAMP_DISJOINT>() as u32,
                    D3D11_ASYNC_GETDATA_DONOTFLUSH.0 as u32,
                )
            };
            if res.is_err() || disjoint_data.Frequency == 0 {
                continue;
            }
            if disjoint_data.Disjoint.as_bool() {
                s.pending = false;
                continue;
            }
            let read_pair = |start_q: &ID3D11Query, end_q: &ID3D11Query, recorded: bool| -> Option<f32> {
                if !recorded { return None; }
                let mut t0 = u64::MAX;
                let mut t1 = u64::MAX;
                let r0 = unsafe {
                    context.GetData(
                        start_q,
                        Some((&mut t0 as *mut u64).cast()),
                        size_of::<u64>() as u32,
                        D3D11_ASYNC_GETDATA_DONOTFLUSH.0 as u32,
                    )
                };
                let r1 = unsafe {
                    context.GetData(
                        end_q,
                        Some((&mut t1 as *mut u64).cast()),
                        size_of::<u64>() as u32,
                        D3D11_ASYNC_GETDATA_DONOTFLUSH.0 as u32,
                    )
                };
                if r0.is_ok() && r1.is_ok() && t0 != u64::MAX && t1 != u64::MAX {
                    Some(((t1.saturating_sub(t0)) as f64 * 1000.0 / disjoint_data.Frequency as f64) as f32)
                } else { None }
            };
            let motion_ms = read_pair(&s.motion_start, &s.motion_end, s.motion_recorded);
            let interp_ms = read_pair(&s.interp_start, &s.interp_end, s.interp_recorded);
            let upscale_ms = read_pair(&s.upscale_start, &s.upscale_end, s.upscale_recorded);
            if (s.motion_recorded && motion_ms.is_none()) || (s.interp_recorded && interp_ms.is_none()) || (s.upscale_recorded && upscale_ms.is_none()) { continue; }
            s.pending = false;
            if self.last_timings.is_none_or(|last| s.generation > last.generation) {
                self.last_timings = Some(GpuTimingSample { generation: s.generation, motion_ms, interpolation_ms: interp_ms, upscale_ms });
            }
        }
    }
}

/// Compiled clean-room D3D11 compute pipeline for spatial upscaling & optical-flow interpolation.
pub struct ComputePipeline {
    cs_nearest: ID3D11ComputeShader,
    cs_bilinear_sharp: ID3D11ComputeShader,
    cs_bicubic_cas: ID3D11ComputeShader,
    cs_spatial_easu: ID3D11ComputeShader,
    cs_nis_sharpen: ID3D11ComputeShader,
    cs_ls1_detail: ID3D11ComputeShader,
    cs_anime4k_restore: ID3D11ComputeShader,
    cs_sgsr_mobile: ID3D11ComputeShader,
    cs_xbr_pixel: ID3D11ComputeShader,
    cs_motion_est: ID3D11ComputeShader,
    cs_motion_est_coarse: ID3D11ComputeShader,
    cs_motion_luma: ID3D11ComputeShader,
    cs_motion_smooth: ID3D11ComputeShader,
    cs_frame_interp: ID3D11ComputeShader,
    cs_sdr_to_scrgb: ID3D11ComputeShader,
    upscale_cb: ID3D11Buffer,
    motion_cb: ID3D11Buffer,
    interp_cb: ID3D11Buffer,
    linear_clamp: ID3D11SamplerState,
    luma_srv: ID3D11ShaderResourceView,
    luma_uav: ID3D11UnorderedAccessView,
    luma_width: u32,
    luma_height: u32,
    coarse_motion_srv: ID3D11ShaderResourceView,
    coarse_motion_uav: ID3D11UnorderedAccessView,
    pub coarse_width: u32,
    pub coarse_height: u32,
    #[cfg_attr(not(test), allow(dead_code))]
    raw_occlusion_tex: ID3D11Texture2D,
    raw_occlusion_srv: ID3D11ShaderResourceView,
    raw_occlusion_uav: ID3D11UnorderedAccessView,
    #[cfg_attr(not(test), allow(dead_code))]
    raw_motion_tex: ID3D11Texture2D,
    raw_motion_srv: ID3D11ShaderResourceView,
    raw_motion_uav: ID3D11UnorderedAccessView,
    pub occlusion_tex: ID3D11Texture2D,
    pub occlusion_srv: ID3D11ShaderResourceView,
    pub occlusion_uav: ID3D11UnorderedAccessView,
    pub motion_tex: ID3D11Texture2D,
    pub motion_srv: ID3D11ShaderResourceView,
    pub motion_uav: ID3D11UnorderedAccessView,
    pub flow_width: u32,
    pub flow_height: u32,
    timer_ring: Mutex<GpuTimerRing>,
}

impl ComputePipeline {
    pub fn new(ctx: &D3d11Context, width: u32, height: u32) -> Result<Self, GpuError> {
        Self::new_with_flow_scale(ctx, width, height, 100)
    }

    /// Creates the compute pipeline with optical-flow grid scaled by `flow_scale_pct` (`25..=100`).
    pub fn new_with_flow_scale(
        ctx: &D3d11Context,
        width: u32,
        height: u32,
        flow_scale_pct: i32,
    ) -> Result<Self, GpuError> {
        let pct = flow_scale_pct.clamp(25, 100) as u32;
        let flow_width = ((width.max(1) * pct / 100) / 4).max(8);
        let flow_height = ((height.max(1) * pct / 100) / 4).max(8);
        let coarse_width = (flow_width / 2).max(4);
        let coarse_height = (flow_height / 2).max(4);
        let (_coarse_motion_tex, coarse_motion_srv, coarse_motion_uav) =
            ctx.create_rw_texture(coarse_width, coarse_height, DXGI_FORMAT_R32G32B32A32_FLOAT)?;
        let luma_width = width.max(1).div_ceil(4);
        let luma_height = height.max(1).div_ceil(4);
        let (_, luma_srv, luma_uav) =
            ctx.create_rw_texture(luma_width, luma_height, DXGI_FORMAT_R32G32_FLOAT)?;
        let (raw_motion_tex, raw_motion_srv, raw_motion_uav) =
            ctx.create_rw_texture(flow_width, flow_height, DXGI_FORMAT_R32G32_FLOAT)?;
        let (raw_occlusion_tex, raw_occlusion_srv, raw_occlusion_uav) =
            ctx.create_rw_texture(flow_width, flow_height, DXGI_FORMAT_R32_FLOAT)?;
        let (motion_tex, motion_srv, motion_uav) =
            ctx.create_rw_texture(flow_width, flow_height, DXGI_FORMAT_R32G32_FLOAT)?;
        let (occlusion_tex, occlusion_srv, occlusion_uav) =
            ctx.create_rw_texture(flow_width, flow_height, DXGI_FORMAT_R32_FLOAT)?;
        // Default occlusion confidence to 1.0 so callers/tests populating `motion_tex` directly still interpolate cleanly.
        let init_occ = vec![1.0f32; (flow_width * flow_height) as usize];
        unsafe {
            ctx.context.UpdateSubresource(
                &occlusion_tex,
                0,
                None,
                init_occ.as_ptr().cast(),
                flow_width * 4,
                0,
            );
            ctx.context.UpdateSubresource(
                &raw_occlusion_tex,
                0,
                None,
                init_occ.as_ptr().cast(),
                flow_width * 4,
                0,
            );
        }
        let timer_ring = Mutex::new(GpuTimerRing::new(&ctx.device)?);

        Ok(Self {
            cs_nearest: ctx.create_compute_shader(NEAREST_DXBC)?,
            cs_bilinear_sharp: ctx.create_compute_shader(BILINEAR_SHARP_DXBC)?,
            cs_bicubic_cas: ctx.create_compute_shader(BICUBIC_CAS_DXBC)?,
            cs_spatial_easu: ctx.create_compute_shader(SPATIAL_EASU_DXBC)?,
            cs_nis_sharpen: ctx.create_compute_shader(NIS_SHARPEN_DXBC)?,
            cs_ls1_detail: ctx.create_compute_shader(LS1_DETAIL_DXBC)?,
            cs_anime4k_restore: ctx.create_compute_shader(ANIME4K_RESTORE_DXBC)?,
            cs_sgsr_mobile: ctx.create_compute_shader(SGSR_MOBILE_DXBC)?,
            cs_xbr_pixel: ctx.create_compute_shader(XBR_PIXEL_DXBC)?,
            cs_motion_est: ctx.create_compute_shader(MOTION_EST_DXBC)?,
            cs_motion_est_coarse: ctx.create_compute_shader(MOTION_EST_COARSE_DXBC)?,
            cs_motion_luma: ctx.create_compute_shader(MOTION_LUMA_DXBC)?,
            cs_motion_smooth: ctx.create_compute_shader(MOTION_SMOOTH_DXBC)?,
            cs_frame_interp: ctx.create_compute_shader(FRAME_INTERP_DXBC)?,
            cs_sdr_to_scrgb: ctx.create_compute_shader(SDR_TO_SCRGB_DXBC)?,
            upscale_cb: ctx.create_constant_buffer::<UpscaleParams>()?,
            motion_cb: ctx.create_constant_buffer::<MotionParams>()?,
            interp_cb: ctx.create_constant_buffer::<InterpParams>()?,
            linear_clamp: ctx.create_linear_clamp_sampler()?,
            luma_srv,
            luma_uav,
            luma_width,
            luma_height,
            coarse_motion_srv,
            coarse_motion_uav,
            coarse_width,
            coarse_height,
            raw_occlusion_tex,
            raw_occlusion_srv,
            raw_occlusion_uav,
            raw_motion_tex,
            raw_motion_srv,
            raw_motion_uav,
            occlusion_tex,
            occlusion_srv,
            occlusion_uav,
            motion_tex,
            motion_srv,
            motion_uav,
            flow_width,
            flow_height,
            timer_ring,
        })
    }

    /// Decodes only encoded SDR captures; genuine FP16 WGC surfaces bypass this dispatch.
    pub fn dispatch_sdr_to_scrgb(&self, ctx: &D3d11Context, input: &ID3D11ShaderResourceView, output: &ID3D11UnorderedAccessView, w: u32, h: u32, white_scale: f32) -> Result<(), GpuError> {
        let params = InterpParams { frame_size: [w,h], alpha: white_scale, ui_threshold: 0.0 };
        ctx.update_constant_buffer(&self.interp_cb, &params)?;
        unsafe {
            ctx.context.CSSetShader(&self.cs_sdr_to_scrgb, None);
            ctx.context.CSSetConstantBuffers(0, Some(&[Some(self.interp_cb.clone())]));
            ctx.context.CSSetShaderResources(0, Some(&[Some(input.clone())]));
            ctx.context.CSSetUnorderedAccessViews(0, 1, Some(&Some(output.clone())), None);
            ctx.context.Dispatch(w.div_ceil(8), h.div_ceil(8), 1);
            ctx.context.CSSetShaderResources(0, Some(&[None]));
            ctx.context.CSSetUnorderedAccessViews(0, 1, Some(&None), None);
        }
        Ok(())
    }

    /// Returns the most recently resolved non-blocking GPU stage timings `(motion_ms, interp_ms, upscale_ms)`.
    pub fn last_gpu_timings(&self) -> Option<(f32, f32, f32)> {
        self.last_gpu_timing_sample().map(|sample| (sample.motion_ms.unwrap_or(0.0), sample.interpolation_ms.unwrap_or(0.0), sample.upscale_ms.unwrap_or(0.0)))
    }

    /// None stages were not dispatched in this generation, rather than unresolved zero timings.
    pub fn last_gpu_timing_sample(&self) -> Option<GpuTimingSample> {
        self.timer_ring.lock().unwrap().last_timings
    }

    #[allow(clippy::too_many_arguments)] // Mirrors the shader's input/output dispatch parameters.
    pub fn dispatch_upscale(
        &self,
        ctx: &D3d11Context,
        scaling_type: ScalingType,
        input_srv: &ID3D11ShaderResourceView,
        output_uav: &ID3D11UnorderedAccessView,
        in_w: u32,
        in_h: u32,
        out_w: u32,
        out_h: u32,
        sharpness: i32,
    ) -> Result<(), GpuError> {
        self.dispatch_upscale_ex(
            ctx,
            scaling_type,
            input_srv,
            output_uav,
            in_w,
            in_h,
            out_w,
            out_h,
            [0, 0, out_w, out_h],
            sharpness,
            0,
            false,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn dispatch_upscale_ex(
        &self,
        ctx: &D3d11Context,
        scaling_type: ScalingType,
        input_srv: &ID3D11ShaderResourceView,
        output_uav: &ID3D11UnorderedAccessView,
        in_w: u32,
        in_h: u32,
        out_w: u32,
        out_h: u32,
        dst_rect: [u32; 4],
        sharpness: i32,
        subtype: u32,
        vrs: bool,
    ) -> Result<(), GpuError> {
        let shader = match scaling_type {
            ScalingType::Nearest | ScalingType::Integer => &self.cs_nearest,
            ScalingType::Off | ScalingType::SharpBilinear => &self.cs_bilinear_sharp,
            ScalingType::BicubicCas => &self.cs_bicubic_cas,
            ScalingType::Fsr => &self.cs_spatial_easu,
            ScalingType::Nis => &self.cs_nis_sharpen,
            ScalingType::Ls1 => &self.cs_ls1_detail,
            ScalingType::Anime4k => &self.cs_anime4k_restore,
            ScalingType::Sgsr => &self.cs_sgsr_mobile,
            ScalingType::Xbr => &self.cs_xbr_pixel,
        };

        let effective_sharpness = if matches!(scaling_type, ScalingType::Off) {
            0.0
        } else {
            sharpness as f32
        };
        let params = UpscaleParams {
            input_size: [in_w.max(1), in_h.max(1)],
            output_size: [out_w.max(1), out_h.max(1)],
            dst_rect,
            sharpness: effective_sharpness,
            mode_flags: u32::from(matches!(scaling_type, ScalingType::Integer)),
            subtype,
            vrs_flag: u32::from(vrs),
        };
        ctx.update_constant_buffer(&self.upscale_cb, &params)?;

        {
            let mut ring = self.timer_ring.lock().unwrap();
            let active = ring.ensure_active(&ctx.context);
            let write_idx = ring.write_idx;
            let slot = &mut ring.slots[write_idx];
            if active { slot.upscale_recorded = true;
            unsafe { ctx.context.End(&slot.upscale_start) }; }
        }
        unsafe {
            ctx.context.CSSetShader(shader, None);
            ctx.context
                .CSSetConstantBuffers(0, Some(&[Some(self.upscale_cb.clone())]));
            ctx.context
                .CSSetShaderResources(0, Some(&[Some(input_srv.clone())]));
            ctx.context
                .CSSetUnorderedAccessViews(0, 1, Some(&Some(output_uav.clone())), None);
            ctx.context
                .CSSetSamplers(0, Some(&[Some(self.linear_clamp.clone())]));
            ctx.context
                .Dispatch(out_w.div_ceil(8), out_h.div_ceil(8), 1);

            ctx.context.CSSetShaderResources(0, Some(&[None]));
            ctx.context
                .CSSetUnorderedAccessViews(0, 1, Some(&None), None);
        }
        {
            let mut ring = self.timer_ring.lock().unwrap();
            let write_idx = ring.write_idx;
            if ring.slots[write_idx].active { unsafe { ctx.context.End(&ring.slots[write_idx].upscale_end) }; }
            ring.finish_frame_and_poll(&ctx.context);
        }
        Ok(())
    }

    pub fn dispatch_motion_est(
        &self,
        ctx: &D3d11Context,
        prev_srv: &ID3D11ShaderResourceView,
        curr_srv: &ID3D11ShaderResourceView,
        w: u32,
        h: u32,
        search_radius: i32,
    ) -> Result<(), GpuError> {
        let clamped_radius = search_radius.clamp(1, 16);
        let coarse_stride: u32 = if clamped_radius >= 8 { 2 } else { 3 };

        {
            let mut ring = self.timer_ring.lock().unwrap();
            let active = ring.ensure_active(&ctx.context);
            let write_idx = ring.write_idx;
            let slot = &mut ring.slots[write_idx];
            if active { slot.motion_recorded = true;
            unsafe { ctx.context.End(&slot.motion_start) }; }
        }

        let use_pyramid = clamped_radius >= 5;
        if use_pyramid {
            let luma_params = MotionParams {
                frame_size: [w, h],
                flow_size: [self.luma_width, self.luma_height],
                search_radius: clamped_radius,
                block_radius: 2,
                pass_mode: 0,
                coarse_stride,
            };
            ctx.update_constant_buffer(&self.motion_cb, &luma_params)?;
            unsafe {
                ctx.context.CSSetShader(&self.cs_motion_luma, None);
                ctx.context.CSSetConstantBuffers(0, Some(&[Some(self.motion_cb.clone())]));
                ctx.context.CSSetShaderResources(0, Some(&[Some(prev_srv.clone()), Some(curr_srv.clone())]));
                ctx.context.CSSetUnorderedAccessViews(0, 1, Some(&Some(self.luma_uav.clone())), None);
                ctx.context.CSSetSamplers(0, Some(&[Some(self.linear_clamp.clone())]));
                ctx.context.Dispatch(self.luma_width.div_ceil(8), self.luma_height.div_ceil(8), 1);
                ctx.context.CSSetUnorderedAccessViews(0, 1, Some(&None), None);
            }
            // Pass 0: Coarse pyramid level into `coarse_motion_uav` for large shifts (> 8 px).
            let coarse_params = MotionParams {
                frame_size: [w, h],
                flow_size: [self.coarse_width, self.coarse_height],
                search_radius: clamped_radius,
                block_radius: 2,
                pass_mode: 0,
                coarse_stride,
            };
            ctx.update_constant_buffer(&self.motion_cb, &coarse_params)?;
            unsafe {
                ctx.context.CSSetShader(&self.cs_motion_est_coarse, None);
                ctx.context
                    .CSSetConstantBuffers(0, Some(&[Some(self.motion_cb.clone())]));
                ctx.context.CSSetShaderResources(
                    0,
                    Some(&[Some(prev_srv.clone()), Some(curr_srv.clone()), None, Some(self.luma_srv.clone())]),
                );
                let coarse_uavs = [None, None, Some(self.coarse_motion_uav.clone())];
                ctx.context
                    .CSSetUnorderedAccessViews(0, 3, Some(coarse_uavs.as_ptr()), None);
                ctx.context.CSSetSamplers(0, Some(&[Some(self.linear_clamp.clone())]));
                ctx.context
                    .Dispatch(self.coarse_width.div_ceil(8), self.coarse_height.div_ceil(8), 1);
                let null_uavs = [None, None, None];
                ctx.context
                    .CSSetUnorderedAccessViews(0, 3, Some(null_uavs.as_ptr()), None);
            }
        }

        // Pass 1 (pyramid fine) or Pass 2 (single-pass fine):
        // outputs `raw_motion_uav` + `raw_occlusion_uav`, then `cs_motion_smooth`
        // filters cross-threadgroup outliers into `motion_uav` + `occlusion_uav`.
        let fine_params = MotionParams {
            frame_size: [w, h],
            flow_size: [self.flow_width, self.flow_height],
            search_radius: clamped_radius,
            block_radius: 2,
            pass_mode: if use_pyramid { 1 } else { 2 },
            coarse_stride,
        };
        ctx.update_constant_buffer(&self.motion_cb, &fine_params)?;
        unsafe {
            ctx.context.CSSetShader(&self.cs_motion_est, None);
            ctx.context
                .CSSetConstantBuffers(0, Some(&[Some(self.motion_cb.clone())]));
            ctx.context.CSSetShaderResources(
                0,
                Some(&[
                    Some(prev_srv.clone()),
                    Some(curr_srv.clone()),
                    if use_pyramid {
                        Some(self.coarse_motion_srv.clone())
                    } else {
                        None
                    },
                ]),
            );
            let raw_uavs = [
                Some(self.raw_motion_uav.clone()),
                Some(self.raw_occlusion_uav.clone()),
            ];
            ctx.context
                .CSSetUnorderedAccessViews(0, 2, Some(raw_uavs.as_ptr()), None);
            ctx.context.CSSetSamplers(0, Some(&[Some(self.linear_clamp.clone())]));
            ctx.context
                .Dispatch(self.flow_width.div_ceil(8), self.flow_height.div_ceil(8), 1);

            ctx.context
                .CSSetShaderResources(0, Some(&[None, None, None, None]));
            let null_uavs = [None, None];
            ctx.context
                .CSSetUnorderedAccessViews(0, 2, Some(null_uavs.as_ptr()), None);
        }
        self.dispatch_motion_smooth(ctx, prev_srv, curr_srv)?;
        {
            let ring = self.timer_ring.lock().unwrap();
            if ring.slots[ring.write_idx].active { unsafe { ctx.context.End(&ring.slots[ring.write_idx].motion_end) }; }
        }
        Ok(())
    }

    fn dispatch_motion_smooth(
        &self,
        ctx: &D3d11Context,
        prev_srv: &ID3D11ShaderResourceView,
        curr_srv: &ID3D11ShaderResourceView,
    ) -> Result<(), GpuError> {
        unsafe {
            ctx.context.CSSetShader(&self.cs_motion_smooth, None);
            ctx.context
                .CSSetConstantBuffers(0, Some(&[Some(self.motion_cb.clone())]));
            ctx.context.CSSetShaderResources(
                0,
                Some(&[
                    Some(self.raw_motion_srv.clone()),
                    Some(self.raw_occlusion_srv.clone()),
                    Some(prev_srv.clone()),
                    Some(curr_srv.clone()),
                ]),
            );
            let uavs = [Some(self.motion_uav.clone()), Some(self.occlusion_uav.clone())];
            ctx.context
                .CSSetUnorderedAccessViews(0, 2, Some(uavs.as_ptr()), None);
            ctx.context
                .CSSetSamplers(0, Some(&[Some(self.linear_clamp.clone())]));
            ctx.context
                .Dispatch(self.flow_width.div_ceil(8), self.flow_height.div_ceil(8), 1);

            ctx.context
                .CSSetShaderResources(0, Some(&[None, None, None, None]));
            let null_uavs = [None, None];
            ctx.context
                .CSSetUnorderedAccessViews(0, 2, Some(null_uavs.as_ptr()), None);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)] // Mirrors the shader's input/output dispatch parameters.
    pub fn dispatch_frame_interp(
        &self,
        ctx: &D3d11Context,
        prev_srv: &ID3D11ShaderResourceView,
        curr_srv: &ID3D11ShaderResourceView,
        out_uav: &ID3D11UnorderedAccessView,
        w: u32,
        h: u32,
        alpha: f32,
    ) -> Result<(), GpuError> {
        self.dispatch_frame_interp_ex(ctx, prev_srv, curr_srv, out_uav, w, h, alpha, 0.0)
    }

    #[allow(clippy::too_many_arguments)] // Mirrors the shader's input/output dispatch parameters.
    pub fn dispatch_frame_interp_ex(
        &self,
        ctx: &D3d11Context,
        prev_srv: &ID3D11ShaderResourceView,
        curr_srv: &ID3D11ShaderResourceView,
        out_uav: &ID3D11UnorderedAccessView,
        w: u32,
        h: u32,
        alpha: f32,
        ui_threshold: f32,
    ) -> Result<(), GpuError> {
        let params = InterpParams {
            frame_size: [w, h],
            alpha: alpha.clamp(0.0, 1.0),
            ui_threshold: ui_threshold.max(0.0),
        };
        ctx.update_constant_buffer(&self.interp_cb, &params)?;

        {
            let mut ring = self.timer_ring.lock().unwrap();
            let active = ring.ensure_active(&ctx.context);
            let write_idx = ring.write_idx;
            let slot = &mut ring.slots[write_idx];
            if active { slot.interp_recorded = true;
            unsafe { ctx.context.End(&slot.interp_start) }; }
        }
        unsafe {
            ctx.context.CSSetShader(&self.cs_frame_interp, None);
            ctx.context
                .CSSetConstantBuffers(0, Some(&[Some(self.interp_cb.clone())]));
            ctx.context.CSSetShaderResources(
                0,
                Some(&[
                    Some(prev_srv.clone()),
                    Some(curr_srv.clone()),
                    Some(self.motion_srv.clone()),
                    Some(self.occlusion_srv.clone()),
                ]),
            );
            ctx.context
                .CSSetUnorderedAccessViews(0, 1, Some(&Some(out_uav.clone())), None);
            ctx.context.CSSetSamplers(0, Some(&[Some(self.linear_clamp.clone())]));
            ctx.context.Dispatch(w.div_ceil(8), h.div_ceil(8), 1);

            ctx.context
                .CSSetShaderResources(0, Some(&[None, None, None, None]));
            ctx.context
                .CSSetUnorderedAccessViews(0, 1, Some(&None), None);
        }
        {
            let ring = self.timer_ring.lock().unwrap();
            if ring.slots[ring.write_idx].active { unsafe { ctx.context.End(&ring.slots[ring.write_idx].interp_end) }; }
        }
        Ok(())
    }
}

fn pick_hardware_adapter(factory: &IDXGIFactory1, gpu_id: i32) -> Result<IDXGIAdapter1, GpuError> {
    if gpu_id < 0 {
        if let Ok(factory6) = factory.cast::<IDXGIFactory6>() {
            let mut idx = 0u32;
            while let Ok(adapter) = unsafe {
                factory6.EnumAdapterByGpuPreference::<IDXGIAdapter1>(
                    idx,
                    DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE,
                )
            } {
                if let Ok(desc) = unsafe { adapter.GetDesc1() } {
                    if (desc.Flags & (DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32)) == 0 {
                        return Ok(adapter);
                    }
                }
                idx += 1;
            }
        }
    }
    let mut first_hw: Option<IDXGIAdapter1> = None;
    let mut hw_idx = 0i32;
    for raw_idx in 0..32 {
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(raw_idx) }) else {
            break;
        };
        let Ok(desc) = (unsafe { adapter.GetDesc1() }) else {
            continue;
        };
        if (desc.Flags & (DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32)) != 0 {
            continue;
        }
        if gpu_id < 0 {
            return Ok(adapter);
        }
        if hw_idx == gpu_id {
            return Ok(adapter);
        }
        if first_hw.is_none() {
            first_hw = Some(adapter);
        }
        hw_idx += 1;
    }
    first_hw.map_or_else(
        || {
            unsafe { factory.EnumAdapters1(0) }.map_err(|e| GpuError::Dxgi(e.to_string()))
        },
        Ok,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_dst_rect_handles_auto_and_custom_modes() {
        assert_eq!(
            compute_dst_rect(1280, 720, 1920, 1080, ScalingMode::Auto, ScalingFitMode::AspectRatio, 1.0),
            [0, 0, 1920, 1080]
        );
        // 4:3 input inside 1920x1080 -> pillarboxed 1440x1080 at offset_x=240
        assert_eq!(
            compute_dst_rect(800, 600, 1920, 1080, ScalingMode::Auto, ScalingFitMode::AspectRatio, 1.0),
            [240, 0, 1440, 1080]
        );
        assert_eq!(
            compute_dst_rect(800, 600, 1920, 1080, ScalingMode::Auto, ScalingFitMode::Fullscreen, 1.0),
            [0, 0, 1920, 1080]
        );
        // Custom 1.5x of 800x600 = 1200x900 centered inside 1920x1080 -> (360, 90)
        assert_eq!(
            compute_dst_rect(800, 600, 1920, 1080, ScalingMode::Custom, ScalingFitMode::AspectRatio, 1.5),
            [360, 90, 1200, 900]
        );
    }

    #[test]
    fn swap_chain_distinguishes_device_loss_from_other_dxgi_errors() {
        use windows::Win32::Graphics::Dxgi::DXGI_ERROR_INVALID_CALL;

        for code in [DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET] {
            assert!(matches!(OverlaySwapChain::swap_chain_error(code.into()), GpuError::DeviceLost(_)));
        }
        assert!(matches!(
            OverlaySwapChain::swap_chain_error(DXGI_ERROR_INVALID_CALL.into()),
            GpuError::Dxgi(_)
        ));
    }

    #[test]
    fn compiled_dxbc_shaders_have_valid_header() {
        for (name, bytes) in [
            ("nearest", NEAREST_DXBC),
            ("bilinear_sharp", BILINEAR_SHARP_DXBC),
            ("bicubic_cas", BICUBIC_CAS_DXBC),
            ("spatial_easu", SPATIAL_EASU_DXBC),
            ("nis_sharpen", NIS_SHARPEN_DXBC),
            ("ls1_detail", LS1_DETAIL_DXBC),
            ("anime4k_restore", ANIME4K_RESTORE_DXBC),
            ("sgsr_mobile", SGSR_MOBILE_DXBC),
            ("xbr_pixel", XBR_PIXEL_DXBC),
            ("motion_est", MOTION_EST_DXBC),
            ("motion_luma", MOTION_LUMA_DXBC),
            ("motion_smooth", MOTION_SMOOTH_DXBC),
            ("frame_interp", FRAME_INTERP_DXBC),
            ("overlay_blend", OVERLAY_BLEND_DXBC),
        ] {
            assert!(bytes.len() > 32, "{name} DXBC too small");
            assert_eq!(&bytes[0..4], b"DXBC", "{name} missing DXBC magic");
        }
    }

    #[test]
    fn d3d11_context_and_compute_pipeline_smoke() {
        let Ok(ctx) = D3d11Context::new(0) else {
            // Headless CI without DXGI adapter
            return;
        };
        let mt: ID3D11Multithread = ctx.context.cast().unwrap();
        assert!(unsafe { mt.GetMultithreadProtected() }.as_bool());
        let cb = ctx.create_constant_buffer::<u32>().unwrap();
        assert!(ctx.update_constant_buffer(&cb, &[0u32; 8]).is_err());
        // A rejected upload must leave the buffer unmapped and usable.
        ctx.update_constant_buffer(&cb, &42u32).unwrap();
        #[repr(C, align(64))]
        #[derive(Clone, Copy)]
        struct Aligned([u32; 16]);
        let aligned_cb = ctx.create_constant_buffer::<Aligned>().unwrap();
        ctx.update_constant_buffer(&aligned_cb, &Aligned([7; 16])).unwrap();
        let pipeline = ComputePipeline::new_with_flow_scale(&ctx, 128, 128, 50).unwrap();
        assert_eq!(pipeline.flow_width, 16);
        assert_eq!(pipeline.flow_height, 16);
        assert_eq!(search_radius_for_fg_size(FrameGenerationSize::Balanced), 8);
        assert_eq!(search_radius_for_fg_size(FrameGenerationSize::Performance), 5);

        let (_in_tex, in_srv, _in_uav) = ctx
            .create_rw_texture(64, 64, DXGI_FORMAT_B8G8R8A8_UNORM)
            .unwrap();
        let (_out_tex, _out_srv, out_uav) = ctx
            .create_rw_texture(128, 128, DXGI_FORMAT_B8G8R8A8_UNORM)
            .unwrap();

        let all_types = [
            ScalingType::Off,
            ScalingType::Ls1,
            ScalingType::Fsr,
            ScalingType::Nis,
            ScalingType::Sgsr,
            ScalingType::BicubicCas,
            ScalingType::Anime4k,
            ScalingType::Xbr,
            ScalingType::SharpBilinear,
            ScalingType::Integer,
            ScalingType::Nearest,
        ];
        for scaling_type in all_types {
            pipeline
                .dispatch_upscale_ex(
                    &ctx,
                    scaling_type,
                    &in_srv,
                    &out_uav,
                    64,
                    64,
                    128,
                    128,
                    [16, 16, 96, 96],
                    3,
                    1,
                    true,
                )
                .unwrap();
        }
        pipeline
            .dispatch_motion_est(&ctx, &in_srv, &in_srv, 64, 64, 8)
            .unwrap();
        pipeline
            .dispatch_frame_interp_ex(&ctx, &in_srv, &in_srv, &out_uav, 64, 64, 0.5, 0.5)
            .unwrap();
    }

    #[test]
    fn motion_and_interpolation_recover_known_translation_on_gpu() {
        use windows::Win32::Graphics::Direct3D11::{
            D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_USAGE_STAGING,
        };

        let Ok(ctx) = D3d11Context::new(0) else {
            eprintln!("Motion readback requires a D3D11 adapter");
            return;
        };
        const SIDE: u32 = 141;
        let pipeline = ComputePipeline::new(&ctx, SIDE, SIDE).unwrap();
        let luminance = |x: i32, y: i32| ((x * 17 + y * 13 + x * y * 3) & 255) as u32;
        let upload = |shift_x: i32, shift_y: i32| {
            let pixels: Vec<u32> = (0..SIDE * SIDE)
                .map(|index| {
                    let x = (index % SIDE) as i32 - shift_x;
                    let y = (index / SIDE) as i32 - shift_y;
                    // A one-pixel HUD line in screen space, outside the translation samples.
                    let gray = if (18..=22).contains(&(index % SIDE)) {
                        if index % SIDE == 20 { 255 } else { 0 }
                    } else if (38..=42).contains(&(index % SIDE)) {
                        // The old warp lands on background while the new sample stays on the object.
                        if shift_x != 0 { 255 } else if index % SIDE == 40 { 128 } else { 0 }
                    } else {
                        luminance(x, y)
                    };
                    0xff00_0000 | (gray * 0x0001_0101)
                })
                .collect();
            let (texture, srv, _) = ctx
                .create_rw_texture(SIDE, SIDE, DXGI_FORMAT_B8G8R8A8_UNORM)
                .unwrap();
            unsafe {
                ctx.context.UpdateSubresource(&texture, 0, None, pixels.as_ptr().cast(), SIDE * 4, 0);
            }
            srv
        };
        let prev = upload(0, 0);
        let curr = upload(4, -2);
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { pipeline.motion_tex.GetDesc(&mut desc) };
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        let sample = |pipeline: &ComputePipeline, curr: &ID3D11ShaderResourceView, x: u32, y: u32| {
            let mut staging_desc = desc;
            staging_desc.Width = pipeline.flow_width;
            staging_desc.Height = pipeline.flow_height;
            let mut staging = None;
            unsafe { ctx.device.CreateTexture2D(&staging_desc, None, Some(&mut staging)).unwrap() };
            let staging = staging.unwrap();
            pipeline.dispatch_motion_est(&ctx, &prev, curr, SIDE, SIDE, 8).unwrap();
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            unsafe {
                ctx.context.CopyResource(&staging, &pipeline.motion_tex);
                ctx.context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).unwrap();
                let row = mapped.pData.cast::<u8>().add(y as usize * mapped.RowPitch as usize);
                let vector = row.cast::<[f32; 2]>().add(x as usize).read_unaligned();
                ctx.context.Unmap(&staging, 0);
                vector
            }
        };
        let low_scale = ComputePipeline::new_with_flow_scale(&ctx, SIDE, SIDE, 25).unwrap();
        // The 25% grid exceeds the tile cache. The 100% grid has a partial thread group.
        for candidate in [&low_scale, &pipeline] {
            let [x, y] = sample(candidate, &curr, candidate.flow_width / 2, candidate.flow_height / 2);
            assert!((x - 4.0).abs() <= 0.5 && (y + 2.0).abs() <= 0.5, "measured ({x}, {y})");
        }
        let (interpolated, _, interp_uav) = ctx
            .create_rw_texture(SIDE, SIDE, DXGI_FORMAT_B8G8R8A8_UNORM)
            .unwrap();
        pipeline
            .dispatch_frame_interp(&ctx, &prev, &curr, &interp_uav, SIDE, SIDE, 0.5)
            .unwrap();
        let interp_desc = D3D11_TEXTURE2D_DESC {
            Width: SIDE,
            Height: SIDE,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            ..desc
        };
        let mut interp_staging = None;
        unsafe { ctx.device.CreateTexture2D(&interp_desc, None, Some(&mut interp_staging)).unwrap() };
        let interp_staging = interp_staging.unwrap();
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        let (mut warped_error, mut unwarped_error) = (0, 0);
        unsafe {
            ctx.context.CopyResource(&interp_staging, &interpolated);
            ctx.context.Map(&interp_staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).unwrap();
            for y in 56..72 {
                let row = mapped.pData.cast::<u8>().add(y as usize * mapped.RowPitch as usize);
                for x in 56..72 {
                    let actual = u32::from(*row.add(x as usize * 4));
                    let expected = luminance(x - 2, y + 1);
                    let unwarped = (luminance(x, y) + luminance(x - 4, y + 2)) / 2;
                    warped_error += expected.abs_diff(actual);
                    unwarped_error += expected.abs_diff(unwarped);
                }
            }
            ctx.context.Unmap(&interp_staging, 0);
        }
        assert!(warped_error * 2 < unwarped_error, "midpoint error {warped_error}, crossfade error {unwarped_error}");
        let [x, y] = sample(&pipeline, &prev, pipeline.flow_width - 2, pipeline.flow_height - 2);
        assert!(x.abs() <= 0.5 && y.abs() <= 0.5, "stationary frame measured ({x}, {y})");
        // Model background motion crossing the static HUD edge.
        let motion = vec![[4.0f32, 0.0]; (pipeline.flow_width * pipeline.flow_height) as usize];
        unsafe {
            ctx.context.UpdateSubresource(
                &pipeline.motion_tex, 0, None, motion.as_ptr().cast(), pipeline.flow_width * 8, 0,
            );
        }
        for (threshold, expected) in [(0.0, 0), (0.2, 255)] {
            pipeline
                .dispatch_frame_interp_ex(&ctx, &prev, &curr, &interp_uav, SIDE, SIDE, 0.5, threshold)
                .unwrap();
            let (hud_pixel, occlusion_pixel) = unsafe {
                ctx.context.CopyResource(&interp_staging, &interpolated);
                ctx.context.Map(&interp_staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).unwrap();
                let row = mapped.pData.cast::<u8>().add(64 * mapped.RowPitch as usize);
                let pixels = (*row.add(20 * 4), *row.add(40 * 4));
                ctx.context.Unmap(&interp_staging, 0);
                pixels
            };
            assert_eq!(hud_pixel, expected, "HUD line at threshold {threshold}");
            assert!((occlusion_pixel as i32 - 191).abs() <= 1, "continuous occlusion fallback at threshold {threshold}: {occlusion_pixel}");
        }
        // Zero motion creates equal spatial scores at both edges of a moving object.
        let object_frame = |left: u32| {
            let pixels: Vec<u32> = (0..SIDE * SIDE).map(|i| {
                let x = i % SIDE;
                if (left..left + 8).contains(&x) { 0xffff_ffff } else { 0xff00_0000 }
            }).collect();
            let (tex, srv, _) = ctx.create_rw_texture(SIDE, SIDE, DXGI_FORMAT_B8G8R8A8_UNORM).unwrap();
            unsafe { ctx.context.UpdateSubresource(&tex, 0, None, pixels.as_ptr().cast(), SIDE * 4, 0) };
            srv
        };
        let object_prev = object_frame(16);
        let object_curr = object_frame(24);
        let zero_motion = vec![[0.0f32; 2]; (pipeline.flow_width * pipeline.flow_height) as usize];
        let zero_conf = vec![0.0f32; zero_motion.len()];
        unsafe {
            ctx.context.UpdateSubresource(&pipeline.motion_tex, 0, None, zero_motion.as_ptr().cast(), pipeline.flow_width * 8, 0);
            ctx.context.UpdateSubresource(&pipeline.occlusion_tex, 0, None, zero_conf.as_ptr().cast(), pipeline.flow_width * 4, 0);
        }
        for alpha in [0.25, 0.49, 0.5, 0.51, 0.75] {
            pipeline.dispatch_frame_interp_ex(&ctx, &object_prev, &object_curr, &interp_uav, SIDE, SIDE, alpha, 0.0).unwrap();
            let (left, right) = unsafe {
                ctx.context.CopyResource(&interp_staging, &interpolated);
                ctx.context.Map(&interp_staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).unwrap();
                let row = mapped.pData.cast::<u8>().add(64 * mapped.RowPitch as usize);
                let pixels = (*row.add(20 * 4), *row.add(28 * 4));
                ctx.context.Unmap(&interp_staging, 0);
                pixels
            };
            if alpha < 0.49 { assert!(left >= 200 && right <= 55); }
            else if alpha > 0.51 { assert!(right >= 200 && left <= 55); }
            else { assert!((left as i32 - 128).abs() <= 6 && (right as i32 - 128).abs() <= 6, "phase continuity {alpha}: ({left},{right})"); }
        }
        // Adjacent surfaces must not interpolate their motion into a third vector.
        let edge_frame = object_frame(63);
        let split_motion: Vec<[f32; 2]> = (0..pipeline.flow_height)
            .flat_map(|_| (0..pipeline.flow_width).map(|x| {
                if x < pipeline.flow_width / 2 { [0.0, 0.0] } else { [8.0, 0.0] }
            })).collect();
        unsafe {
            ctx.context.UpdateSubresource(&pipeline.motion_tex, 0, None, split_motion.as_ptr().cast(), pipeline.flow_width * 8, 0);
        }
        pipeline.dispatch_frame_interp_ex(&ctx, &edge_frame, &edge_frame, &interp_uav, SIDE, SIDE, 0.5, 0.0).unwrap();
        let edge = unsafe {
            ctx.context.CopyResource(&interp_staging, &interpolated);
            ctx.context.Map(&interp_staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).unwrap();
            let pixel = *mapped.pData.cast::<u8>().add(64 * mapped.RowPitch as usize + 63 * 4);
            ctx.context.Unmap(&interp_staging, 0);
            pixel
        };
        assert_eq!(edge, 255, "motion edge must retain the left surface vector");
        eprintln!("D3D11 readback on {}: midpoint error {warped_error}, crossfade error {unwarped_error}", ctx.caps.adapter_name);
    }

    #[test]
    fn swapchain_compute_uav_fps_overlay_and_present_smoke() {
        use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassExW, WINDOW_EX_STYLE,
            WNDCLASSEXW, WS_POPUP,
        };

        unsafe extern "system" fn wndproc(
            hwnd: HWND,
            msg: u32,
            wparam: WPARAM,
            lparam: LPARAM,
        ) -> LRESULT {
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }

        let Ok(ctx) = D3d11Context::new(0) else {
            return;
        };

        let cls_name = w!("FlowScaleGpuTestWnd");
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            lpszClassName: cls_name,
            ..Default::default()
        };
        unsafe {
            RegisterClassExW(&wc);
        }
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                cls_name,
                w!("Test"),
                WS_POPUP,
                0,
                0,
                128,
                128,
                None,
                None,
                None,
                None,
            )
        }
        .expect("CreateWindowExW failed");

        let sc = OverlaySwapChain::new(&ctx, hwnd, 128, 128, 3, false)
            .expect("OverlaySwapChain::new failed");
        let pipeline = ComputePipeline::new(&ctx, 128, 128).unwrap();
        let (in_tex, in_srv, _in_uav) = ctx
            .create_rw_texture(64, 64, DXGI_FORMAT_B8G8R8A8_UNORM)
            .unwrap();
        let source_pixels = [0xff36_783cu32; 64 * 64];
        unsafe {
            ctx.context.UpdateSubresource(&in_tex, 0, None, source_pixels.as_ptr().cast(), 64 * 4, 0);
        }
        let bb_uav = sc.back_buffer_uav(&ctx).expect("back_buffer_uav failed");
        pipeline
            .dispatch_upscale(
                &ctx,
                ScalingType::Fsr,
                &in_srv,
                &bb_uav,
                64,
                64,
                128,
                128,
                5,
            )
            .expect("dispatch_upscale to backbuffer UAV failed");
        drop(bb_uav);

        let bb = sc.back_buffer().unwrap();
        let fps = FpsOverlay::from_config(&FpsCounterConfig {
            scale: 0.9,
            color: 0x00FF_8800,
            font: "Consolas".to_owned(),
            offset_x: 4,
            offset_y: 4,
            show_captured: true,
        })
        .expect("FpsOverlay::from_config failed");
        let cursor = CursorSnapshot {
            visible: true,
            screen_x: 10,
            screen_y: 10,
            hotspot_x: 1,
            hotspot_y: 1,
            width: 8,
            height: 8,
            rgba: vec![255; 8 * 8 * 4],
        };
        use windows::Win32::Graphics::Direct3D11::{
            D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_USAGE_STAGING,
        };
        let pixels = vec![0xff00_0000u32; 128 * 128];
        unsafe {
            ctx.context.UpdateSubresource(&bb, 0, None, pixels.as_ptr().cast(), 128 * 4, 0);
        }
        fps.draw_cursor(&bb, sc.format, &cursor, 32.0, 32.0, 2.0)
            .expect("offscreen cursor draw failed");
        fps.draw_cursor(&bb, sc.format, &cursor, 64.125, 64.125, 0.125)
            .expect("downscaled cursor draw failed");
        let mut staging_desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { bb.GetDesc(&mut staging_desc) };
        staging_desc.Usage = D3D11_USAGE_STAGING;
        staging_desc.BindFlags = 0;
        staging_desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        let mut staging = None;
        unsafe { ctx.device.CreateTexture2D(&staging_desc, None, Some(&mut staging)).unwrap() };
        let staging = staging.unwrap();
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        let samples = unsafe {
            ctx.context.CopyResource(&staging, &bb);
            ctx.context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).unwrap();
            let sample = |x: usize, y: usize| {
                *mapped.pData.cast::<u8>().add(y * mapped.RowPitch as usize + x * 4)
            };
            let values = [sample(29, 32), sample(30, 32), sample(32, 29), sample(32, 30),
                sample(45, 32), sample(46, 32), sample(64, 64), sample(65, 64),
                sample(64, 65)];
            ctx.context.Unmap(&staging, 0);
            values
        };
        assert_eq!(samples, [0, 255, 0, 255, 255, 0, 255, 0, 0]);
        fps.draw_overlays(&bb, sc.format, Some((60, 120, 4.0, 4.0)), Some((&cursor, 32.0, 32.0, 1.5)))
            .expect("FpsOverlay::draw_overlays failed");
        sc.present(SyncMode::Off).expect("present failed");
        drop(bb);
        sc.resize(192, 96).expect("resize failed");
        let resized = sc.back_buffer().unwrap();
        let mut desc = staging_desc;
        unsafe { resized.GetDesc(&mut desc) };
        assert_eq!((desc.Width, desc.Height), (192, 96));
        let resized_uav = sc.back_buffer_uav(&ctx).unwrap();
        pipeline
            .dispatch_upscale(&ctx, ScalingType::Nearest, &in_srv, &resized_uav, 64, 64, 192, 96, 0)
            .expect("redraw from the retained source failed");
        staging_desc.Width = desc.Width;
        staging_desc.Height = desc.Height;
        let mut resized_staging = None;
        let redraw_pixel = unsafe {
            ctx.device.CreateTexture2D(&staging_desc, None, Some(&mut resized_staging)).unwrap();
            let staging = resized_staging.unwrap();
            ctx.context.CopyResource(&staging, &resized);
            ctx.context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).unwrap();
            let row = mapped.pData.cast::<u8>().add(48 * mapped.RowPitch as usize);
            let pixel = row.cast::<u32>().add(96).read_unaligned();
            ctx.context.Unmap(&staging, 0);
            pixel
        };
        assert_eq!(redraw_pixel, source_pixels[0]);
        drop((resized, resized_uav));
        sc.present(SyncMode::Off).expect("present after resize failed");
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }

    #[test]
    fn present_params_selects_tearing_only_for_sync_off_when_supported() {
        assert_eq!(
            OverlaySwapChain::present_params(SyncMode::Off, true),
            (0, DXGI_PRESENT_ALLOW_TEARING)
        );
        assert_eq!(
            OverlaySwapChain::present_params(SyncMode::Off, false),
            (0, DXGI_PRESENT(0))
        );
        assert_eq!(
            OverlaySwapChain::present_params(SyncMode::Default, true),
            (0, DXGI_PRESENT(0))
        );
        assert_eq!(
            OverlaySwapChain::present_params(SyncMode::Default, false),
            (0, DXGI_PRESENT(0))
        );
        assert_eq!(
            OverlaySwapChain::present_params(SyncMode::Vsync1, true),
            (1, DXGI_PRESENT(0))
        );
        assert_eq!(
            OverlaySwapChain::present_params(SyncMode::Vsync2, true),
            (2, DXGI_PRESENT(0))
        );
        assert_eq!(
            OverlaySwapChain::present_params(SyncMode::Vsync3, false),
            (3, DXGI_PRESENT(0))
        );
        assert_eq!(
            OverlaySwapChain::present_params(SyncMode::Vsync4, true),
            (4, DXGI_PRESENT(0))
        );
    }

    #[test]
    fn overlay_swapchain_tearing_flag_and_wait_latency() {
        use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassExW, WINDOW_EX_STYLE,
            WNDCLASSEXW, WS_POPUP,
        };

        unsafe extern "system" fn test_wnd_proc(
            hwnd: HWND,
            msg: u32,
            wparam: WPARAM,
            lparam: LPARAM,
        ) -> LRESULT {
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        let ctx = D3d11Context::new(0).expect("D3d11Context::new(0) failed");
        let cls_name = w!("FlowScaleGpuTearingTest");
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(test_wnd_proc),
            lpszClassName: cls_name,
            ..Default::default()
        };
        unsafe {
            RegisterClassExW(&wc);
        }
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                cls_name,
                w!("TearingTest"),
                WS_POPUP,
                0,
                0,
                64,
                64,
                None,
                None,
                None,
                None,
            )
        }
        .expect("CreateWindowExW failed");

        let sc = OverlaySwapChain::new(&ctx, hwnd, 64, 64, 1, false)
            .expect("OverlaySwapChain::new failed");
        let desc = unsafe { sc.swap_chain.GetDesc1() }.expect("GetDesc1 failed");
        assert_ne!(
            desc.Flags & (DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 as u32),
            0
        );
        assert_eq!(
            (desc.Flags & (DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING.0 as u32)) != 0,
            sc.tearing_supported()
        );
        assert_eq!(sc.wait_for_frame_latency(16), FrameLatencyWait::Signaled);
        sc.present(SyncMode::Off).expect("present(SyncMode::Off) failed");
        sc.set_max_frame_latency(2).expect("set_max_frame_latency(2) failed");
        sc.resize(96, 96).expect("resize failed");
        let resized_desc = unsafe { sc.swap_chain.GetDesc1() }.expect("GetDesc1 after resize failed");
        assert_eq!(
            (resized_desc.Flags & (DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING.0 as u32)) != 0,
            sc.tearing_supported()
        );
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }

    #[test]
    fn large_shift_pyramid_motion_and_hud_invariance_on_gpu() {
        use windows::Win32::Graphics::Direct3D11::{
            D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_USAGE_STAGING,
        };

        let Ok(ctx) = D3d11Context::new(0) else {
            eprintln!("Large-shift pyramid test requires a D3D11 adapter");
            return;
        };
        const SIDE: u32 = 256;
        // Exercise the 50% flow scale packed fp16 shared-memory fast path.
        let pipeline = ComputePipeline::new_with_flow_scale(&ctx, SIDE, SIDE, 50).unwrap();

        let hash2 = |gx: i32, gy: i32| -> f32 {
            let mut h = (gx.wrapping_mul(374_761_393)) ^ (gy.wrapping_mul(668_265_263));
            h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
            ((h ^ (h >> 16)) & 0xFFFF) as f32 / 65535.0
        };
        let noise_octave = |x: i32, y: i32, cell: f32| -> f32 {
            let fx = x as f32 / cell;
            let fy = y as f32 / cell;
            let gx = fx.floor() as i32;
            let gy = fy.floor() as i32;
            let tx = fx - gx as f32;
            let ty = fy - gy as f32;
            let u = tx * tx * (3.0 - 2.0 * tx);
            let v = ty * ty * (3.0 - 2.0 * ty);
            let a = hash2(gx, gy);
            let b = hash2(gx + 1, gy);
            let c = hash2(gx, gy + 1);
            let d = hash2(gx + 1, gy + 1);
            a + (b - a) * u + (c - a) * v + (a - b - c + d) * u * v
        };
        let pattern = |x: i32, y: i32| -> u32 {
            let n = 0.65 * noise_octave(x, y, 24.0) + 0.35 * noise_octave(x, y, 12.0);
            (n * 255.0).clamp(0.0, 255.0) as u32
        };
        let is_hud = |ix: u32, iy: u32| -> bool {
            // Static high-contrast crosshair near top-left (away from moving center test ROI).
            ((28..=52).contains(&ix) && (39..=41).contains(&iy))
                || ((39..=41).contains(&ix) && (28..=52).contains(&iy))
        };
        let upload = |shift_x: i32, shift_y: i32| {
            let pixels: Vec<u32> = (0..SIDE * SIDE)
                .map(|index| {
                    let ix = index % SIDE;
                    let iy = index / SIDE;
                    let gray = if is_hud(ix, iy) {
                        255
                    } else {
                        pattern(ix as i32 - shift_x, iy as i32 - shift_y)
                    };
                    0xff00_0000 | (gray * 0x0001_0101)
                })
                .collect();
            let (tex, srv, _) = ctx
                .create_rw_texture(SIDE, SIDE, DXGI_FORMAT_B8G8R8A8_UNORM)
                .unwrap();
            unsafe {
                ctx.context
                    .UpdateSubresource(&tex, 0, None, pixels.as_ptr().cast(), SIDE * 4, 0);
            }
            srv
        };

        let prev = upload(0, 0);
        let curr = upload(14, -10);
        pipeline
            .dispatch_motion_est(&ctx, &prev, &curr, SIDE, SIDE, search_radius_for_fg_size(FrameGenerationSize::Balanced))
            .expect("dispatch_motion_est failed");

        let mut mv_desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { pipeline.motion_tex.GetDesc(&mut mv_desc) };
        mv_desc.Usage = D3D11_USAGE_STAGING;
        mv_desc.BindFlags = 0;
        mv_desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        let mut mv_staging = None;
        unsafe { ctx.device.CreateTexture2D(&mv_desc, None, Some(&mut mv_staging)).unwrap() };
        let mv_staging = mv_staging.unwrap();
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        let [mx, my] = unsafe {
            ctx.context.CopyResource(&mv_staging, &pipeline.motion_tex);
            ctx.context
                .Map(&mv_staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .unwrap();
            let row = mapped
                .pData
                .cast::<u8>()
                .add((pipeline.flow_height / 2) as usize * mapped.RowPitch as usize);
            let v = row
                .cast::<[f32; 2]>()
                .add((pipeline.flow_width / 2) as usize)
                .read_unaligned();
            ctx.context.Unmap(&mv_staging, 0);
            v
        };
        assert!(
            (mx - 14.0).abs() <= 0.6 && (my + 10.0).abs() <= 0.6,
            "expected (+14, -10) large-shift MV, got ({mx}, {my})"
        );

        let (interp_tex, _, interp_uav) = ctx
            .create_rw_texture(SIDE, SIDE, DXGI_FORMAT_B8G8R8A8_UNORM)
            .unwrap();
        pipeline
            .dispatch_frame_interp_ex(&ctx, &prev, &curr, &interp_uav, SIDE, SIDE, 0.5, 0.2)
            .expect("dispatch_frame_interp_ex failed");

        let mut img_desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { interp_tex.GetDesc(&mut img_desc) };
        img_desc.Usage = D3D11_USAGE_STAGING;
        img_desc.BindFlags = 0;
        img_desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        let mut img_staging = None;
        unsafe { ctx.device.CreateTexture2D(&img_desc, None, Some(&mut img_staging)).unwrap() };
        let img_staging = img_staging.unwrap();

        let (mut moving_se, mut moving_n, mut hud_se, mut hud_n) = (0.0f64, 0u64, 0.0f64, 0u64);
        unsafe {
            ctx.context.CopyResource(&img_staging, &interp_tex);
            ctx.context
                .Map(&img_staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .unwrap();
            for y in 96..160u32 {
                let row = mapped.pData.cast::<u8>().add(y as usize * mapped.RowPitch as usize);
                for x in 96..160u32 {
                    let actual = f64::from(*row.add(x as usize * 4));
                    let expected = f64::from(pattern(x as i32 - 7, y as i32 + 5));
                    let err = actual - expected;
                    moving_se += err * err;
                    moving_n += 1;
                }
            }
            for y in 28..=52u32 {
                let row = mapped.pData.cast::<u8>().add(y as usize * mapped.RowPitch as usize);
                for x in 28..=52u32 {
                    if is_hud(x, y) {
                        let actual = f64::from(*row.add(x as usize * 4));
                        let err = actual - 255.0;
                        hud_se += err * err;
                        hud_n += 1;
                    }
                }
            }
            ctx.context.Unmap(&img_staging, 0);
        }

        let moving_mse = (moving_se / moving_n as f64).max(1e-6);
        let moving_psnr = 10.0 * (255.0f64 * 255.0 / moving_mse).log10();
        if moving_psnr <= 28.0 {
            unsafe {
                ctx.context.CopyResource(&mv_staging, &pipeline.motion_tex);
                ctx.context.Map(&mv_staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).unwrap();
                let mut bad = Vec::new();
                for fy in 12..20usize {
                    let row = mapped.pData.cast::<u8>().add(fy * mapped.RowPitch as usize);
                    for fx in 12..20usize {
                        let [vx, vy] = row.cast::<[f32; 2]>().add(fx).read_unaligned();
                        if (vx - 14.0).abs() > 1.0 || (vy + 10.0).abs() > 1.0 {
                            bad.push((fx, fy, vx, vy));
                        }
                    }
                }
                ctx.context.Unmap(&mv_staging, 0);
                panic!("expected moving PSNR > 28 dB, got {moving_psnr:.2} dB; bad MVs in 12..20: {bad:?}");
            }
        }

        let hud_mse = (hud_se / hud_n as f64).max(1e-6);
        let hud_psnr = 10.0 * (255.0f64 * 255.0 / hud_mse).log10();
        assert!(hud_psnr > 35.0, "expected HUD invariance PSNR > 35 dB, got {hud_psnr:.2} dB");

        for (shift_x, shift_y) in [(20, 2), (30, -6), (46, 4), (48, 0), (-48, 4), (62, -6), (64, 0), (-64, -8), (68, 0), (-68, 4)] {
            let curr = upload(shift_x, shift_y);
            pipeline.dispatch_motion_est(&ctx, &prev, &curr, SIDE, SIDE, 8).unwrap();
            pipeline.dispatch_frame_interp_ex(&ctx, &prev, &curr, &interp_uav, SIDE, SIDE, 0.5, 0.2).unwrap();
            let (mut se, mut good) = (0.0f64, 0u32);
            let mut bad_mvs = Vec::new();
            unsafe {
                ctx.context.CopyResource(&mv_staging, &pipeline.motion_tex);
                ctx.context.Map(&mv_staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).unwrap();
                for fy in 12..20usize {
                    let row = mapped.pData.cast::<u8>().add(fy * mapped.RowPitch as usize);
                    for fx in 12..20usize {
                        let [x, y] = row.cast::<[f32; 2]>().add(fx).read_unaligned();
                        if (x - shift_x as f32).abs() <= 1.0 && (y - shift_y as f32).abs() <= 1.0 {
                            good += 1;
                        } else {
                            bad_mvs.push((fx, fy, x, y));
                        }
                    }
                }
                ctx.context.Unmap(&mv_staging, 0);
                ctx.context.CopyResource(&img_staging, &interp_tex);
                ctx.context.Map(&img_staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).unwrap();
                for y in 96..160u32 {
                    let row = mapped.pData.cast::<u8>().add(y as usize * mapped.RowPitch as usize);
                    for x in 96..160u32 {
                        let actual = f64::from(*row.add(x as usize * 4));
                        let expected = f64::from(pattern(x as i32 - shift_x / 2, y as i32 - shift_y / 2));
                        se += (actual - expected).powi(2);
                    }
                }
                ctx.context.Unmap(&img_staging, 0);
            }
            let psnr = 10.0 * (255.0f64.powi(2) / (se / 4096.0).max(1e-6)).log10();
            eprintln!("shift ({shift_x}, {shift_y}): {good}/64 good vectors, {psnr:.2} dB");
            let min_good = if (shift_x, shift_y) == (20, 2) { 64 } else { 48 };
            assert!(
                good >= min_good && psnr > 30.0,
                "shift ({shift_x}, {shift_y}): {good}/64 good vectors (expected >= {min_good}), {psnr:.2} dB; bad MVs: {bad_mvs:?}"
            );
        }
    }

    #[test]
    #[ignore = "manual 1080p GPU timing"]
    fn balanced_1080p_motion_interp_timing() {
        let ctx = D3d11Context::new(0).expect("D3D11 adapter required");
        let (w, h) = match std::env::var("FLOWSCALE_BENCH_HEIGHT").as_deref() {
            Ok("720") => (1280, 720),
            Ok("1080") | Err(std::env::VarError::NotPresent) => (1920, 1080),
            _ => panic!("benchmark height must be 720 or 1080"),
        };
        let flow_scale = std::env::var("FLOWSCALE_BENCH_FLOW_SCALE")
            .map_or(50, |value| value.parse::<i32>().expect("integer flow scale"));
        assert!((25..=100).contains(&flow_scale));
        let preset = match std::env::var("FLOWSCALE_BENCH_PRESET").as_deref() {
            Ok("Performance") => FrameGenerationSize::Performance,
            Ok("Balanced") | Err(std::env::VarError::NotPresent) => FrameGenerationSize::Balanced,
            _ => panic!("benchmark preset must be Balanced or Performance"),
        };
        let pipeline = ComputePipeline::new_with_flow_scale(&ctx, w, h, flow_scale).unwrap();
        let (prev_tex, prev, _) = ctx.create_rw_texture(w, h, DXGI_FORMAT_B8G8R8A8_UNORM).unwrap();
        let (curr_tex, curr, _) = ctx.create_rw_texture(w, h, DXGI_FORMAT_B8G8R8A8_UNORM).unwrap();
        let (_, interp, interp_uav) = ctx.create_rw_texture(w, h, DXGI_FORMAT_B8G8R8A8_UNORM).unwrap();
        let (_, _, out) = ctx.create_rw_texture(w, h, DXGI_FORMAT_B8G8R8A8_UNORM).unwrap();
        let pattern = |x: u32, y: u32| {
            let n = (x / 12).wrapping_mul(2654435761) ^ (y / 12).wrapping_mul(2246822519);
            0xff00_0000 | (n & 0x00ff_ffff)
        };
        let pixels: Vec<u32> = (0..w*h).map(|i| pattern(i % w, i / w)).collect();
        let shifted: Vec<u32> = (0..w*h).map(|i| pattern((i % w).wrapping_sub(48), (i / w).wrapping_sub(4))).collect();
        unsafe {
            ctx.context.UpdateSubresource(&prev_tex, 0, None, pixels.as_ptr().cast(), w * 4, 0);
            ctx.context.UpdateSubresource(&curr_tex, 0, None, shifted.as_ptr().cast(), w * 4, 0);
        }
        let mut timings = Vec::new();
        let mut last_generation = 0;
        for frame in 0..36 {
            pipeline.dispatch_motion_est(&ctx, &prev, &curr, w, h, search_radius_for_fg_size(preset)).unwrap();
            pipeline.dispatch_frame_interp_ex(&ctx, &prev, &curr, &interp_uav, w, h, 0.5, 0.2).unwrap();
            pipeline.dispatch_upscale(&ctx, ScalingType::Nearest, &interp, &out, w, h, w, h, 0).unwrap();
            unsafe { ctx.context.Flush() };
            std::thread::sleep(std::time::Duration::from_millis(30));
            if frame >= 8 {
                if let Some(GpuTimingSample {
                    generation,
                    motion_ms: Some(motion),
                    interpolation_ms: Some(interpolation),
                    ..
                }) = pipeline.last_gpu_timing_sample().filter(|sample| sample.generation > last_generation)
                {
                    last_generation = generation;
                    timings.push((motion + interpolation, motion, interpolation));
                }
            }
        }
        assert!(timings.len() >= 24, "at least 24 completed GPU timestamp samples required, got {}", timings.len());
        timings.sort_by(|a, b| a.0.total_cmp(&b.0));
        let (median, motion, interpolation) = timings[timings.len() / 2];
        eprintln!(
            "{}: {h}p {preset:?} {flow_scale}% motion + interpolation median {median:.3} ms (motion {motion:.3}, interpolation {interpolation:.3}), p95 {:.3} ms, range {:.3} to {:.3} ms ({} samples)",
            ctx.caps.adapter_name, timings[(timings.len() * 95).div_ceil(100) - 1].0, timings[0].0, timings[timings.len() - 1].0, timings.len()
        );
        if h == 1080 && flow_scale == 50 && matches!(preset, FrameGenerationSize::Balanced) {
            assert!(median <= 1.80, "Balanced 50% motion + interpolation exceeds 1.80 ms budget");
        }
    }

    #[test]
    fn gpu_timestamp_profiler_3_frame_ring_is_non_blocking() {
        let Ok(ctx) = D3d11Context::new(0) else {
            eprintln!("GPU timestamp profiler test requires a D3D11 adapter");
            return;
        };
        let pipeline = ComputePipeline::new_with_flow_scale(&ctx, 128, 128, 50).unwrap();
        let (_prev_tex, prev_srv, _) = ctx
            .create_rw_texture(128, 128, DXGI_FORMAT_B8G8R8A8_UNORM)
            .unwrap();
        let (_curr_tex, curr_srv, _) = ctx
            .create_rw_texture(128, 128, DXGI_FORMAT_B8G8R8A8_UNORM)
            .unwrap();
        let (_interp_tex, interp_srv, interp_uav) = ctx
            .create_rw_texture(128, 128, DXGI_FORMAT_B8G8R8A8_UNORM)
            .unwrap();
        let (_out_tex, _, out_uav) = ctx
            .create_rw_texture(256, 256, DXGI_FORMAT_B8G8R8A8_UNORM)
            .unwrap();

        for _ in 0..8 {
            pipeline
                .dispatch_motion_est(&ctx, &prev_srv, &curr_srv, 128, 128, 8)
                .unwrap();
            pipeline
                .dispatch_frame_interp_ex(&ctx, &prev_srv, &curr_srv, &interp_uav, 128, 128, 0.5, 0.15)
                .unwrap();
            pipeline
                .dispatch_upscale(&ctx, ScalingType::Fsr, &interp_srv, &out_uav, 128, 128, 256, 256, 5)
                .unwrap();
            unsafe { ctx.context.Flush() };
            std::thread::sleep(std::time::Duration::from_millis(2));
        }

        let timings = pipeline
            .last_gpu_timings()
            .expect("expected resolved GPU timings after 8 flushed frames");
        assert!(timings.0.is_finite() && timings.0 >= 0.0);
        assert!(timings.1.is_finite() && timings.1 >= 0.0);
        assert!(timings.2.is_finite() && timings.2 >= 0.0);
        assert!(timings.0 + timings.1 + timings.2 > 0.0);
    }

    #[test]
    fn fps_overlay_caches_offscreen_textures_and_backbuffer_uav_resizes_cleanly() {
        use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
        use windows::Win32::Graphics::Direct3D11::{
            D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_USAGE_STAGING,
        };
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassExW, WINDOW_EX_STYLE,
            WNDCLASSEXW, WS_POPUP,
        };

        unsafe extern "system" fn wndproc(
            hwnd: HWND,
            msg: u32,
            wparam: WPARAM,
            lparam: LPARAM,
        ) -> LRESULT {
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        let Ok(ctx) = D3d11Context::new(0) else {
            return;
        };
        let cls_name = w!("FlowScaleGpuFpsCacheTest");
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            lpszClassName: cls_name,
            ..Default::default()
        };
        unsafe { RegisterClassExW(&wc) };
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                cls_name,
                w!("CacheTest"),
                WS_POPUP,
                0,
                0,
                160,
                96,
                None,
                None,
                None,
                None,
            )
        }
        .expect("CreateWindowExW failed");

        let sc = OverlaySwapChain::new(&ctx, hwnd, 160, 96, 2, false).unwrap();
        let uav1 = sc.back_buffer_uav(&ctx).unwrap();
        let uav2 = sc.back_buffer_uav(&ctx).unwrap();
        assert_eq!(uav1.as_raw(), uav2.as_raw(), "expected cached backbuffer UAV");
        drop((uav1, uav2));

        let bb = sc.back_buffer().unwrap();
        let zero_pixels = vec![0xff00_0000u32; 160 * 96];
        unsafe {
            ctx.context
                .UpdateSubresource(&bb, 0, None, zero_pixels.as_ptr().cast(), 160 * 4, 0);
        }

        let fps = FpsOverlay::new(18.0).unwrap();
        let cursor = CursorSnapshot {
            visible: true,
            screen_x: 0,
            screen_y: 0,
            hotspot_x: 0,
            hotspot_y: 0,
            width: 4,
            height: 4,
            rgba: vec![255; 4 * 4 * 4],
        };
        fps.draw_overlays(&bb, sc.format, Some((70, 140, 8.0, 8.0)), Some((&cursor, 80.0, 40.0, 1.0)))
            .unwrap();
        let (fps_ptr_1, cursor_ptr_1) = {
            let guard = fps.scratch.lock().unwrap();
            let g = guard.gpu.as_ref().unwrap();
            (
                g.fps_cache.as_ref().unwrap()._tex.as_raw() as usize,
                g.cursor_cache.as_ref().unwrap().tex.as_raw() as usize,
            )
        };

        // Repeat with identical FPS and cursor: both offscreen textures must be reused.
        fps.draw_overlays(&bb, sc.format, Some((70, 140, 8.0, 8.0)), Some((&cursor, 84.0, 42.0, 1.0)))
            .unwrap();
        // Change FPS value: offscreen fps texture stays the same COM object, only re-rendered.
        fps.draw_overlays(&bb, sc.format, Some((71, 142, 8.0, 8.0)), Some((&cursor, 84.0, 42.0, 1.0)))
            .unwrap();
        let (fps_ptr_2, cursor_ptr_2, last_fps) = {
            let guard = fps.scratch.lock().unwrap();
            let g = guard.gpu.as_ref().unwrap();
            (
                g.fps_cache.as_ref().unwrap()._tex.as_raw() as usize,
                g.cursor_cache.as_ref().unwrap().tex.as_raw() as usize,
                g.fps_cache.as_ref().unwrap().last_fps,
            )
        };
        assert_eq!(fps_ptr_1, fps_ptr_2);
        assert_eq!(cursor_ptr_1, cursor_ptr_2);
        assert_eq!(last_fps, Some((71, 142)));

        // Verify FPS pixels actually composited onto the backbuffer.
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { bb.GetDesc(&mut desc) };
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        let mut staging = None;
        unsafe { ctx.device.CreateTexture2D(&desc, None, Some(&mut staging)).unwrap() };
        let staging = staging.unwrap();
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        let mut lit_fps_pixels = 0u32;
        unsafe {
            ctx.context.CopyResource(&staging, &bb);
            ctx.context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).unwrap();
            for y in 8..36usize {
                let row = mapped.pData.cast::<u8>().add(y * mapped.RowPitch as usize);
                for x in 8..76usize {
                    let b = *row.add(x * 4);
                    let g = *row.add(x * 4 + 1);
                    let r = *row.add(x * 4 + 2);
                    if r > 32 || g > 32 || b > 32 {
                        lit_fps_pixels += 1;
                    }
                }
            }
            ctx.context.Unmap(&staging, 0);
        }
        assert!(lit_fps_pixels > 20, "expected rendered FPS pixels on backbuffer, got {lit_fps_pixels}");

        drop(bb);
        sc.resize(200, 120).expect("resize must succeed after cached UAV release and FpsOverlay draws");
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }

    #[test]
    fn motion_smooth_and_interp_suppress_isolated_bad_mv_and_disocclusion() {
        use windows::Win32::Graphics::Direct3D11::{
            D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_USAGE_STAGING,
        };

        let Ok(ctx) = D3d11Context::new(0) else {
            return;
        };
        const SIDE: u32 = 128;
        let pipeline = ComputePipeline::new_with_flow_scale(&ctx, SIDE, SIDE, 100).unwrap();
        let bg = |x: i32, y: i32| -> u32 {
            let v = (((x as f32 * 0.14).sin() * 0.5 + 0.5) * 140.0
                + ((y as f32 * 0.11).cos() * 0.5 + 0.5) * 90.0)
                .clamp(0.0, 255.0) as u32;
            0xff00_0000 | (v * 0x0001_0101)
        };
        let upload_shift = |sx: i32, sy: i32, occlude_box: bool| {
            let pixels: Vec<u32> = (0..SIDE * SIDE)
                .map(|i| {
                    let x = (i % SIDE) as i32;
                    let y = (i / SIDE) as i32;
                    if occlude_box && (80..96).contains(&x) && (80..96).contains(&y) {
                        0xffff_ffff
                    } else {
                        bg(x - sx, y - sy)
                    }
                })
                .collect();
            let (tex, srv, _) = ctx
                .create_rw_texture(SIDE, SIDE, DXGI_FORMAT_B8G8R8A8_UNORM)
                .unwrap();
            unsafe {
                ctx.context
                    .UpdateSubresource(&tex, 0, None, pixels.as_ptr().cast(), SIDE * 4, 0);
            }
            srv
        };

        let prev = upload_shift(0, 0, false);
        let curr = upload_shift(8, 0, true);

        // Seed raw motion with uniform (+8, 0) except an isolated bad outlier at (8, 8) (threadgroup seam).
        let fw = pipeline.flow_width as usize;
        let fh = pipeline.flow_height as usize;
        let mut raw_mv = vec![[8.0f32, 0.0f32]; fw * fh];
        raw_mv[8 * fw + 8] = [-16.0, 12.0];
        let raw_occ = vec![1.0f32; fw * fh];
        let params = MotionParams {
            frame_size: [SIDE, SIDE],
            flow_size: [pipeline.flow_width, pipeline.flow_height],
            search_radius: 8,
            block_radius: 2,
            pass_mode: 1,
            coarse_stride: 2,
        };
        ctx.update_constant_buffer(&pipeline.motion_cb, &params).unwrap();
        unsafe {
            ctx.context.UpdateSubresource(
                &pipeline.raw_motion_tex,
                0,
                None,
                raw_mv.as_ptr().cast(),
                pipeline.flow_width * 8,
                0,
            );
            ctx.context.UpdateSubresource(
                &pipeline.raw_occlusion_tex,
                0,
                None,
                raw_occ.as_ptr().cast(),
                pipeline.flow_width * 4,
                0,
            );
        }
        pipeline.dispatch_motion_smooth(&ctx, &prev, &curr).unwrap();

        let mut mv_desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { pipeline.motion_tex.GetDesc(&mut mv_desc) };
        mv_desc.Usage = D3D11_USAGE_STAGING;
        mv_desc.BindFlags = 0;
        mv_desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        let mut mv_staging = None;
        unsafe { ctx.device.CreateTexture2D(&mv_desc, None, Some(&mut mv_staging)).unwrap() };
        let mv_staging = mv_staging.unwrap();
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        let smoothed_seam = unsafe {
            ctx.context.CopyResource(&mv_staging, &pipeline.motion_tex);
            ctx.context.Map(&mv_staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).unwrap();
            let row = mapped.pData.cast::<u8>().add(8 * mapped.RowPitch as usize);
            let v = row.cast::<[f32; 2]>().add(8).read_unaligned();
            ctx.context.Unmap(&mv_staging, 0);
            v
        };
        assert!(
            (smoothed_seam[0] - 8.0).abs() <= 0.5 && smoothed_seam[1].abs() <= 0.5,
            "expected cross-threadgroup outlier (8,8) repaired to ~(8,0), got {smoothed_seam:?}"
        );

        // Now force a broken MV + low occlusion over the disoccluded white box (80..96) and verify
        // frame_interp falls back toward nearest real frame rather than warping garbage.
        let mut broken_mv = vec![[8.0f32, 0.0f32]; fw * fh];
        let mut broken_occ = vec![1.0f32; fw * fh];
        for fy in 20..24 {
            for fx in 20..24 {
                broken_mv[fy * fw + fx] = [-20.0, -20.0];
                broken_occ[fy * fw + fx] = 0.0;
            }
        }
        unsafe {
            ctx.context.UpdateSubresource(
                &pipeline.motion_tex,
                0,
                None,
                broken_mv.as_ptr().cast(),
                pipeline.flow_width * 8,
                0,
            );
            ctx.context.UpdateSubresource(
                &pipeline.occlusion_tex,
                0,
                None,
                broken_occ.as_ptr().cast(),
                pipeline.flow_width * 4,
                0,
            );
        }
        let (interp_tex, _, interp_uav) = ctx
            .create_rw_texture(SIDE, SIDE, DXGI_FORMAT_B8G8R8A8_UNORM)
            .unwrap();
        let mut img_desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { interp_tex.GetDesc(&mut img_desc) };
        img_desc.Usage = D3D11_USAGE_STAGING;
        img_desc.BindFlags = 0;
        img_desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        let mut img_staging = None;
        unsafe { ctx.device.CreateTexture2D(&img_desc, None, Some(&mut img_staging)).unwrap() };
        let img_staging = img_staging.unwrap();

        pipeline
            .dispatch_frame_interp_ex(&ctx, &prev, &curr, &interp_uav, SIDE, SIDE, 0.75, 0.0)
            .unwrap();
        let center_75 = unsafe {
            ctx.context.CopyResource(&img_staging, &interp_tex);
            ctx.context.Map(&img_staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).unwrap();
            let px = *mapped.pData.cast::<u8>().add(88 * mapped.RowPitch as usize + 88 * 4);
            ctx.context.Unmap(&img_staging, 0);
            px
        };
        assert!(
            center_75 >= 210,
            "expected broken-MV disocclusion at alpha=0.75 to recover nearestReal (curr=255), got {center_75}"
        );
    }

    #[test]
    fn translucent_antialiased_hud_over_panning_background_stays_unwarped() {
        use windows::Win32::Graphics::Direct3D11::{
            D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_USAGE_STAGING,
        };

        let Ok(ctx) = D3d11Context::new(0) else {
            return;
        };
        const SIDE: u32 = 256;
        let pipeline = ComputePipeline::new_with_flow_scale(&ctx, SIDE, SIDE, 50).unwrap();

        let hash2 = |gx: i32, gy: i32| -> f32 {
            let mut h = (gx.wrapping_mul(374_761_393)) ^ (gy.wrapping_mul(668_265_263));
            h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
            ((h ^ (h >> 16)) & 0xFFFF) as f32 / 65535.0
        };
        let noise_octave = |x: i32, y: i32, cell: f32| -> f32 {
            let fx = x as f32 / cell;
            let fy = y as f32 / cell;
            let gx = fx.floor() as i32;
            let gy = fy.floor() as i32;
            let tx = fx - gx as f32;
            let ty = fy - gy as f32;
            let u = tx * tx * (3.0 - 2.0 * tx);
            let v = ty * ty * (3.0 - 2.0 * ty);
            let a = hash2(gx, gy);
            let b = hash2(gx + 1, gy);
            let c = hash2(gx, gy + 1);
            let d = hash2(gx + 1, gy + 1);
            a + (b - a) * u + (c - a) * v + (a - b - c + d) * u * v
        };
        let bg_pattern = |x: i32, y: i32| -> f32 {
            let n = 0.65 * noise_octave(x, y, 24.0) + 0.35 * noise_octave(x, y, 12.0);
            (n * 180.0).clamp(16.0, 196.0)
        };
        // Semi-transparent 2px-stroke HUD bar (alpha=0.55) with 1px AA halo (alpha=0.28).
        let hud_alpha = |ix: u32, iy: u32| -> f32 {
            if (28..=56).contains(&ix) && (38..=39).contains(&iy) {
                0.55
            } else if (28..=56).contains(&ix) && (iy == 37 || iy == 40) {
                0.28
            } else {
                0.0
            }
        };
        let upload = |shift_x: i32, shift_y: i32| -> (ID3D11ShaderResourceView, Vec<u8>) {
            let mut luma_bytes = Vec::with_capacity((SIDE * SIDE) as usize);
            let pixels: Vec<u32> = (0..SIDE * SIDE)
                .map(|idx| {
                    let ix = idx % SIDE;
                    let iy = idx / SIDE;
                    let bg = bg_pattern(ix as i32 - shift_x, iy as i32 - shift_y);
                    let a = hud_alpha(ix, iy);
                    let val = (a * 255.0 + (1.0 - a) * bg).round().clamp(0.0, 255.0) as u8;
                    luma_bytes.push(val);
                    0xff00_0000 | (u32::from(val) * 0x0001_0101)
                })
                .collect();
            let (tex, srv, _) = ctx
                .create_rw_texture(SIDE, SIDE, DXGI_FORMAT_B8G8R8A8_UNORM)
                .unwrap();
            unsafe {
                ctx.context
                    .UpdateSubresource(&tex, 0, None, pixels.as_ptr().cast(), SIDE * 4, 0);
            }
            (srv, luma_bytes)
        };

        let (prev, _) = upload(0, 0);
        let (curr, curr_bytes) = upload(12, 0);
        pipeline
            .dispatch_motion_est(&ctx, &prev, &curr, SIDE, SIDE, 8)
            .unwrap();
        let (interp_tex, _, interp_uav) = ctx
            .create_rw_texture(SIDE, SIDE, DXGI_FORMAT_B8G8R8A8_UNORM)
            .unwrap();
        pipeline
            .dispatch_frame_interp_ex(&ctx, &prev, &curr, &interp_uav, SIDE, SIDE, 0.5, 0.5)
            .unwrap();

        let mut img_desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { interp_tex.GetDesc(&mut img_desc) };
        img_desc.Usage = D3D11_USAGE_STAGING;
        img_desc.BindFlags = 0;
        img_desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        let mut img_staging = None;
        unsafe { ctx.device.CreateTexture2D(&img_desc, None, Some(&mut img_staging)).unwrap() };
        let img_staging = img_staging.unwrap();
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();

        let (mut hud_se, mut hud_n, mut moving_se, mut moving_n) = (0.0f64, 0u64, 0.0f64, 0u64);
        unsafe {
            ctx.context.CopyResource(&img_staging, &interp_tex);
            ctx.context
                .Map(&img_staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .unwrap();
            for iy in 38..=39u32 {
                let row = mapped.pData.cast::<u8>().add(iy as usize * mapped.RowPitch as usize);
                for ix in 30..=54u32 {
                    let actual = f64::from(*row.add(ix as usize * 4));
                    let expected = f64::from(curr_bytes[(iy * SIDE + ix) as usize]);
                    let err = actual - expected;
                    hud_se += err * err;
                    hud_n += 1;
                }
            }
            for iy in 128..192u32 {
                let row = mapped.pData.cast::<u8>().add(iy as usize * mapped.RowPitch as usize);
                for ix in 128..192u32 {
                    let actual = f64::from(*row.add(ix as usize * 4));
                    let expected = f64::from(bg_pattern(ix as i32 - 6, iy as i32));
                    let err = actual - expected;
                    moving_se += err * err;
                    moving_n += 1;
                }
            }
            ctx.context.Unmap(&img_staging, 0);
        }
        let hud_psnr = 10.0 * (255.0f64.powi(2) / (hud_se / hud_n as f64).max(1e-6)).log10();
        let moving_psnr =
            10.0 * (255.0f64.powi(2) / (moving_se / moving_n as f64).max(1e-6)).log10();
        eprintln!("translucent HUD PSNR={hud_psnr:.2} dB, panning background PSNR={moving_psnr:.2} dB");
        assert!(hud_psnr >= 38.0, "expected translucent HUD PSNR >= 38 dB, got {hud_psnr:.2} dB");
        assert!(moving_psnr >= 30.0, "expected moving background PSNR >= 30 dB, got {moving_psnr:.2} dB");
    }

    #[test]
    fn moving_foreground_strip_over_stationary_background_keeps_motion_without_holes() {
        use windows::Win32::Graphics::Direct3D11::{
            D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_USAGE_STAGING,
        };

        let Ok(ctx) = D3d11Context::new(0) else {
            return;
        };
        const SIDE: u32 = 256;
        let pipeline = ComputePipeline::new_with_flow_scale(&ctx, SIDE, SIDE, 50).unwrap();

        let bg_pattern = |x: u32, y: u32| -> u8 {
            let v = 24
                + ((x.wrapping_mul(11) ^ y.wrapping_mul(17)) & 31)
                + ((((x / 4) + (y / 4)) * 9) & 31)
                + (((x + y * 2) * 3) & 15);
            v.min(110) as u8
        };
        let fg_pattern = |lx: u32, ly: u32| -> u8 {
            let v = 160
                + ((lx.wrapping_mul(13) + ly.wrapping_mul(7)) & 31)
                + (((lx ^ ly) * 5) & 31);
            v.min(235) as u8
        };
        let is_hud = |x: u32, y: u32| (24..56).contains(&x) && (24..36).contains(&y);

        let upload = |fg_left: u32| {
            let pixels: Vec<u32> = (0..SIDE * SIDE)
                .map(|idx| {
                    let x = idx % SIDE;
                    let y = idx / SIDE;
                    let gray = if is_hud(x, y) {
                        255u32
                    } else if (fg_left..fg_left + 28).contains(&x) && (96..160).contains(&y) {
                        u32::from(fg_pattern(x - fg_left, y - 96))
                    } else {
                        u32::from(bg_pattern(x, y))
                    };
                    0xff00_0000 | (gray * 0x0001_0101)
                })
                .collect();
            let (tex, srv, _) = ctx
                .create_rw_texture(SIDE, SIDE, DXGI_FORMAT_B8G8R8A8_UNORM)
                .unwrap();
            unsafe {
                ctx.context
                    .UpdateSubresource(&tex, 0, None, pixels.as_ptr().cast(), SIDE * 4, 0);
            }
            srv
        };

        let prev = upload(96);
        let curr = upload(114); // (+18, 0) moving textured foreground strip
        pipeline
            .dispatch_motion_est(
                &ctx,
                &prev,
                &curr,
                SIDE,
                SIDE,
                search_radius_for_fg_size(FrameGenerationSize::Balanced),
            )
            .unwrap();

        let (interp_tex, _, interp_uav) = ctx
            .create_rw_texture(SIDE, SIDE, DXGI_FORMAT_B8G8R8A8_UNORM)
            .unwrap();
        pipeline
            .dispatch_frame_interp_ex(&ctx, &prev, &curr, &interp_uav, SIDE, SIDE, 0.5, 0.2)
            .unwrap();

        let mut img_desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { interp_tex.GetDesc(&mut img_desc) };
        img_desc.Usage = D3D11_USAGE_STAGING;
        img_desc.BindFlags = 0;
        img_desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        let mut img_staging = None;
        unsafe { ctx.device.CreateTexture2D(&img_desc, None, Some(&mut img_staging)).unwrap() };
        let img_staging = img_staging.unwrap();
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();

        let (mut fg_se, mut fg_n, mut fg_holes) = (0.0f64, 0u64, 0u32);
        let (mut hud_se, mut hud_n) = (0.0f64, 0u64);
        let (mut bg_se, mut bg_n) = (0.0f64, 0u64);
        unsafe {
            ctx.context.CopyResource(&img_staging, &interp_tex);
            ctx.context
                .Map(&img_staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .unwrap();
            // Midpoint foreground strip sits at [105..133, 96..160]; check interior [111..127, 104..152].
            for y in 104..152u32 {
                let row = mapped.pData.cast::<u8>().add(y as usize * mapped.RowPitch as usize);
                for x in 111..127u32 {
                    let actual = *row.add(x as usize * 4);
                    let expected = fg_pattern(x - 105, y - 96);
                    if actual < 130 {
                        fg_holes += 1;
                    }
                    let err = f64::from(actual) - f64::from(expected);
                    fg_se += err * err;
                    fg_n += 1;
                }
            }
            for y in 26..34u32 {
                let row = mapped.pData.cast::<u8>().add(y as usize * mapped.RowPitch as usize);
                for x in 28..52u32 {
                    let actual = f64::from(*row.add(x as usize * 4));
                    let err = actual - 255.0;
                    hud_se += err * err;
                    hud_n += 1;
                }
            }
            for y in 192..224u32 {
                let row = mapped.pData.cast::<u8>().add(y as usize * mapped.RowPitch as usize);
                for x in 128..192u32 {
                    let actual = f64::from(*row.add(x as usize * 4));
                    let err = actual - f64::from(bg_pattern(x, y));
                    bg_se += err * err;
                    bg_n += 1;
                }
            }
            ctx.context.Unmap(&img_staging, 0);
        }

        let fg_psnr = 10.0 * (255.0f64.powi(2) / (fg_se / fg_n as f64).max(1e-6)).log10();
        let hud_psnr = 10.0 * (255.0f64.powi(2) / (hud_se / hud_n as f64).max(1e-6)).log10();
        let bg_psnr = 10.0 * (255.0f64.powi(2) / (bg_se / bg_n as f64).max(1e-6)).log10();
        eprintln!(
            "moving FG strip (+18,0) PSNR={fg_psnr:.2} dB (holes={fg_holes}), HUD PSNR={hud_psnr:.2} dB, stationary BG PSNR={bg_psnr:.2} dB"
        );
        assert_eq!(fg_holes, 0, "moving foreground collapsed to stationary background holes ({fg_holes} px)");
        assert!(fg_psnr >= 30.0, "expected moving foreground strip PSNR >= 30 dB, got {fg_psnr:.2} dB");
        assert!(hud_psnr >= 40.0, "expected HUD invariance PSNR >= 40 dB, got {hud_psnr:.2} dB");
        assert!(bg_psnr >= 38.0, "expected stationary background PSNR >= 38 dB, got {bg_psnr:.2} dB");
    }
}

#[cfg(test)]
mod stage2_tests;
