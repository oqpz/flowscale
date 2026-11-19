//! CPU-only Vulkan frame interpolation through an explicitly loaded software driver.

use ash::{vk, Entry};
use std::{ffi::CStr, path::Path, sync::Arc};

pub type Result<T> = std::result::Result<T, String>;

fn validate_dimensions(width: u32, height: u32) -> Result<()> {
    if width == 0 || height == 0 || width > 8192 || height > 8192 {
        return Err("Invalid CPU frame dimensions".into());
    }
    Ok(())
}

struct Device {
    _entry: Entry,
    instance: ash::Instance,
    physical: vk::PhysicalDevice,
    device: ash::Device,
    queue: vk::Queue,
    pool: vk::CommandPool,
    command: vk::CommandBuffer,
    memory: vk::PhysicalDeviceMemoryProperties,
    name: String,
}

impl Device {
    fn new(path: &Path) -> Result<Arc<Self>> {
        if !path.is_absolute() {
            return Err("CPU Vulkan runtime path must be absolute".into());
        }
        unsafe {
            let entry = Entry::load_from(path).map_err(|e| format!("CPU Vulkan runtime: {e}"))?;
            let app = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_1);
            let extensions = [
                ash::khr::surface::NAME.as_ptr(),
                ash::khr::win32_surface::NAME.as_ptr(),
            ];
            let instance = entry
                .create_instance(
                    &vk::InstanceCreateInfo::default()
                        .application_info(&app)
                        .enabled_extension_names(&extensions),
                    None,
                )
                .map_err(|e| format!("CPU Vulkan instance: {e}"))?;
            let setup = (|| {
                let physical = instance
                    .enumerate_physical_devices()
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .find(|&p| {
                        instance.get_physical_device_properties(p).device_type
                            == vk::PhysicalDeviceType::CPU
                    })
                    .ok_or("The selected Vulkan runtime exposes no CPU device")?;
                let properties = instance.get_physical_device_properties(physical);
                let family = instance
                    .get_physical_device_queue_family_properties(physical)
                    .iter()
                    .position(|q| q.queue_flags.contains(vk::QueueFlags::COMPUTE))
                    .ok_or("CPU Vulkan compute queue unavailable")?
                    as u32;
                let priorities = [1.0];
                let queues = [vk::DeviceQueueCreateInfo::default()
                    .queue_family_index(family)
                    .queue_priorities(&priorities)];
                let available = instance.get_physical_device_features(physical);
                if available.shader_storage_image_extended_formats == 0 {
                    return Err("CPU driver lacks required storage image formats".into());
                }
                let features = vk::PhysicalDeviceFeatures::default()
                    .shader_storage_image_extended_formats(true);
                let device_extensions = [ash::khr::swapchain::NAME.as_ptr()];
                let device = instance
                    .create_device(
                        physical,
                        &vk::DeviceCreateInfo::default()
                            .queue_create_infos(&queues)
                            .enabled_features(&features)
                            .enabled_extension_names(&device_extensions),
                        None,
                    )
                    .map_err(|e| e.to_string())?;
                let pool = match device.create_command_pool(
                    &vk::CommandPoolCreateInfo::default()
                        .queue_family_index(family)
                        .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                    None,
                ) {
                    Ok(pool) => pool,
                    Err(e) => {
                        device.destroy_device(None);
                        return Err(e.to_string());
                    }
                };
                let command = match device.allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default()
                        .command_pool(pool)
                        .level(vk::CommandBufferLevel::PRIMARY)
                        .command_buffer_count(1),
                ) {
                    Ok(commands) => commands[0],
                    Err(e) => {
                        device.destroy_command_pool(pool, None);
                        device.destroy_device(None);
                        return Err(e.to_string());
                    }
                };
                Ok((
                    physical,
                    device,
                    pool,
                    command,
                    CStr::from_ptr(properties.device_name.as_ptr())
                        .to_string_lossy()
                        .into_owned(),
                ))
            })();
            let (physical, device, pool, command, name) = match setup {
                Ok(result) => result,
                Err(e) => {
                    instance.destroy_instance(None);
                    return Err(e);
                }
            };
            Ok(Arc::new(Self {
                memory: instance.get_physical_device_memory_properties(physical),
                queue: device.get_device_queue(
                    instance
                        .get_physical_device_queue_family_properties(physical)
                        .iter()
                        .position(|q| q.queue_flags.contains(vk::QueueFlags::COMPUTE))
                        .unwrap() as u32,
                    0,
                ),
                _entry: entry,
                instance,
                physical,
                device,
                pool,
                command,
                name,
            }))
        }
    }

    fn memory_type(&self, bits: u32, flags: vk::MemoryPropertyFlags) -> Result<u32> {
        (0..self.memory.memory_type_count)
            .find(|&i| {
                bits & (1 << i) != 0
                    && self.memory.memory_types[i as usize]
                        .property_flags
                        .contains(flags)
            })
            .ok_or_else(|| format!("CPU Vulkan memory type unavailable: {flags:?}"))
    }

    fn submit(&self, record: impl FnOnce(vk::CommandBuffer)) -> Result<()> {
        unsafe {
            self.device
                .reset_command_buffer(self.command, vk::CommandBufferResetFlags::empty())
                .map_err(|e| e.to_string())?;
            self.device
                .begin_command_buffer(
                    self.command,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .map_err(|e| e.to_string())?;
            record(self.command);
            self.device
                .end_command_buffer(self.command)
                .map_err(|e| e.to_string())?;
            let commands = [self.command];
            self.device
                .queue_submit(
                    self.queue,
                    &[vk::SubmitInfo::default().command_buffers(&commands)],
                    vk::Fence::null(),
                )
                .map_err(|e| e.to_string())?;
            self.device
                .queue_wait_idle(self.queue)
                .map_err(|e| e.to_string())
        }
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        unsafe {
            let _ = self.device.device_wait_idle();
            self.device.destroy_command_pool(self.pool, None);
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}

pub fn probe_runtime(path: &Path) -> Result<String> {
    let device = Device::new(path)?;
    Ok(device.name.clone())
}

struct Buffer {
    gpu: Arc<Device>,
    handle: vk::Buffer,
    memory: vk::DeviceMemory,
    size: u64,
}

impl Buffer {
    fn new(gpu: &Arc<Device>, size: u64) -> Result<Self> {
        let mut out = Self {
            gpu: gpu.clone(),
            handle: vk::Buffer::null(),
            memory: vk::DeviceMemory::null(),
            size,
        };
        unsafe {
            out.handle = gpu
                .device
                .create_buffer(
                    &vk::BufferCreateInfo::default().size(size).usage(
                        vk::BufferUsageFlags::TRANSFER_SRC
                            | vk::BufferUsageFlags::TRANSFER_DST
                            | vk::BufferUsageFlags::UNIFORM_BUFFER,
                    ),
                    None,
                )
                .map_err(|e| e.to_string())?;
            let req = gpu.device.get_buffer_memory_requirements(out.handle);
            let kind = gpu.memory_type(
                req.memory_type_bits,
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
            )?;
            out.memory = gpu
                .device
                .allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(req.size)
                        .memory_type_index(kind),
                    None,
                )
                .map_err(|e| e.to_string())?;
            gpu.device
                .bind_buffer_memory(out.handle, out.memory, 0)
                .map_err(|e| e.to_string())?;
        }
        Ok(out)
    }

    fn write(&self, bytes: &[u8]) -> Result<()> {
        if bytes.len() as u64 > self.size {
            return Err("CPU upload exceeds allocation".into());
        }
        unsafe {
            let p = self
                .gpu
                .device
                .map_memory(self.memory, 0, self.size, vk::MemoryMapFlags::empty())
                .map_err(|e| e.to_string())?;
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), p.cast(), bytes.len());
            self.gpu.device.unmap_memory(self.memory);
        }
        Ok(())
    }

    fn read(&self) -> Result<Vec<u8>> {
        unsafe {
            let p = self
                .gpu
                .device
                .map_memory(self.memory, 0, self.size, vk::MemoryMapFlags::empty())
                .map_err(|e| e.to_string())?;
            let bytes = std::slice::from_raw_parts(p.cast(), self.size as usize).to_vec();
            self.gpu.device.unmap_memory(self.memory);
            Ok(bytes)
        }
    }
}

impl Drop for Buffer {
    fn drop(&mut self) {
        unsafe {
            self.gpu.device.destroy_buffer(self.handle, None);
            self.gpu.device.free_memory(self.memory, None);
        }
    }
}

struct Image {
    gpu: Arc<Device>,
    handle: vk::Image,
    memory: vk::DeviceMemory,
    view: vk::ImageView,
    width: u32,
    height: u32,
}

fn color_range() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .level_count(1)
        .layer_count(1)
}

fn color_layers() -> vk::ImageSubresourceLayers {
    vk::ImageSubresourceLayers::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .layer_count(1)
}

fn memory_barrier(gpu: &Device, cmd: vk::CommandBuffer) {
    unsafe {
        gpu.device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::DependencyFlags::empty(),
            &[vk::MemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::MEMORY_WRITE)
                .dst_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE)],
            &[],
            &[],
        );
    }
}

impl Image {
    fn new(gpu: &Arc<Device>, width: u32, height: u32, format: vk::Format) -> Result<Self> {
        let mut out = Self {
            gpu: gpu.clone(),
            handle: vk::Image::null(),
            memory: vk::DeviceMemory::null(),
            view: vk::ImageView::null(),
            width,
            height,
        };
        unsafe {
            out.handle = gpu
                .device
                .create_image(
                    &vk::ImageCreateInfo::default()
                        .image_type(vk::ImageType::TYPE_2D)
                        .format(format)
                        .extent(vk::Extent3D {
                            width,
                            height,
                            depth: 1,
                        })
                        .mip_levels(1)
                        .array_layers(1)
                        .samples(vk::SampleCountFlags::TYPE_1)
                        .tiling(vk::ImageTiling::OPTIMAL)
                        .usage(
                            vk::ImageUsageFlags::SAMPLED
                                | vk::ImageUsageFlags::STORAGE
                                | vk::ImageUsageFlags::TRANSFER_SRC
                                | vk::ImageUsageFlags::TRANSFER_DST,
                        ),
                    None,
                )
                .map_err(|e| e.to_string())?;
            let req = gpu.device.get_image_memory_requirements(out.handle);
            let kind = gpu.memory_type(req.memory_type_bits, vk::MemoryPropertyFlags::empty())?;
            out.memory = gpu
                .device
                .allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(req.size)
                        .memory_type_index(kind),
                    None,
                )
                .map_err(|e| e.to_string())?;
            gpu.device
                .bind_image_memory(out.handle, out.memory, 0)
                .map_err(|e| e.to_string())?;
            out.view = gpu
                .device
                .create_image_view(
                    &vk::ImageViewCreateInfo::default()
                        .image(out.handle)
                        .view_type(vk::ImageViewType::TYPE_2D)
                        .format(format)
                        .subresource_range(color_range()),
                    None,
                )
                .map_err(|e| e.to_string())?;
            gpu.submit(|cmd| {
                gpu.device.cmd_pipeline_barrier(
                    cmd,
                    vk::PipelineStageFlags::TOP_OF_PIPE,
                    vk::PipelineStageFlags::ALL_COMMANDS,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[vk::ImageMemoryBarrier::default()
                        .old_layout(vk::ImageLayout::UNDEFINED)
                        .new_layout(vk::ImageLayout::GENERAL)
                        .dst_access_mask(
                            vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE,
                        )
                        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                        .image(out.handle)
                        .subresource_range(color_range())],
                )
            })?;
        }
        Ok(out)
    }

    fn copy_region(&self) -> vk::BufferImageCopy {
        vk::BufferImageCopy::default()
            .image_subresource(color_layers())
            .image_extent(vk::Extent3D {
                width: self.width,
                height: self.height,
                depth: 1,
            })
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        unsafe {
            self.gpu.device.destroy_image_view(self.view, None);
            self.gpu.device.destroy_image(self.handle, None);
            self.gpu.device.free_memory(self.memory, None);
        }
    }
}

struct Kernel {
    gpu: Arc<Device>,
    pipeline: vk::Pipeline,
    layout: vk::PipelineLayout,
    descriptors: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    set: vk::DescriptorSet,
}

impl Kernel {
    fn new(
        gpu: &Arc<Device>,
        code: &[u8],
        sampled: &[&Image],
        storage: &[&Image],
        constants: &Buffer,
        sampler: vk::Sampler,
    ) -> Result<Self> {
        let mut out = Self {
            gpu: gpu.clone(),
            pipeline: vk::Pipeline::null(),
            layout: vk::PipelineLayout::null(),
            descriptors: vk::DescriptorSetLayout::null(),
            pool: vk::DescriptorPool::null(),
            set: vk::DescriptorSet::null(),
        };
        let mut bindings = Vec::new();
        for (start, count, kind) in [
            (0, sampled.len(), vk::DescriptorType::SAMPLED_IMAGE),
            (8, storage.len(), vk::DescriptorType::STORAGE_IMAGE),
            (16, 1, vk::DescriptorType::UNIFORM_BUFFER),
            (24, 1, vk::DescriptorType::SAMPLER),
        ] {
            for index in 0..count {
                bindings.push(
                    vk::DescriptorSetLayoutBinding::default()
                        .binding(start + index as u32)
                        .descriptor_type(kind)
                        .descriptor_count(1)
                        .stage_flags(vk::ShaderStageFlags::COMPUTE),
                );
            }
        }
        unsafe {
            out.descriptors = gpu
                .device
                .create_descriptor_set_layout(
                    &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                    None,
                )
                .map_err(|e| e.to_string())?;
            let layouts = [out.descriptors];
            out.layout = gpu
                .device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default().set_layouts(&layouts),
                    None,
                )
                .map_err(|e| e.to_string())?;
            let words =
                ash::util::read_spv(&mut std::io::Cursor::new(code)).map_err(|e| e.to_string())?;
            let module = gpu
                .device
                .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&words), None)
                .map_err(|e| e.to_string())?;
            let result = gpu.device.create_compute_pipelines(
                vk::PipelineCache::null(),
                &[vk::ComputePipelineCreateInfo::default()
                    .layout(out.layout)
                    .stage(
                        vk::PipelineShaderStageCreateInfo::default()
                            .stage(vk::ShaderStageFlags::COMPUTE)
                            .module(module)
                            .name(c"main"),
                    )],
                None,
            );
            gpu.device.destroy_shader_module(module, None);
            out.pipeline = result.map_err(|(_, e)| e.to_string())?[0];
            let sizes = [
                vk::DescriptorPoolSize {
                    ty: vk::DescriptorType::SAMPLED_IMAGE,
                    descriptor_count: sampled.len().max(1) as u32,
                },
                vk::DescriptorPoolSize {
                    ty: vk::DescriptorType::STORAGE_IMAGE,
                    descriptor_count: storage.len().max(1) as u32,
                },
                vk::DescriptorPoolSize {
                    ty: vk::DescriptorType::UNIFORM_BUFFER,
                    descriptor_count: 1,
                },
                vk::DescriptorPoolSize {
                    ty: vk::DescriptorType::SAMPLER,
                    descriptor_count: 1,
                },
            ];
            out.pool = gpu
                .device
                .create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(1)
                        .pool_sizes(&sizes),
                    None,
                )
                .map_err(|e| e.to_string())?;
            out.set = gpu
                .device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(out.pool)
                        .set_layouts(&layouts),
                )
                .map_err(|e| e.to_string())?[0];
            for (start, images, kind) in [
                (0, sampled, vk::DescriptorType::SAMPLED_IMAGE),
                (8, storage, vk::DescriptorType::STORAGE_IMAGE),
            ] {
                for (i, image) in images.iter().enumerate() {
                    gpu.device.update_descriptor_sets(
                        &[vk::WriteDescriptorSet::default()
                            .dst_set(out.set)
                            .dst_binding(start + i as u32)
                            .descriptor_type(kind)
                            .image_info(&[vk::DescriptorImageInfo::default()
                                .image_view(image.view)
                                .image_layout(vk::ImageLayout::GENERAL)])],
                        &[],
                    );
                }
            }
            gpu.device.update_descriptor_sets(
                &[vk::WriteDescriptorSet::default()
                    .dst_set(out.set)
                    .dst_binding(16)
                    .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                    .buffer_info(&[vk::DescriptorBufferInfo::default()
                        .buffer(constants.handle)
                        .range(constants.size)])],
                &[],
            );
            gpu.device.update_descriptor_sets(
                &[vk::WriteDescriptorSet::default()
                    .dst_set(out.set)
                    .dst_binding(24)
                    .descriptor_type(vk::DescriptorType::SAMPLER)
                    .image_info(&[vk::DescriptorImageInfo::default().sampler(sampler)])],
                &[],
            );
        }
        Ok(out)
    }

    fn dispatch(&self, cmd: vk::CommandBuffer, width: u32, height: u32) {
        unsafe {
            self.gpu
                .device
                .cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, self.pipeline);
            self.gpu.device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::COMPUTE,
                self.layout,
                0,
                &[self.set],
                &[],
            );
            self.gpu
                .device
                .cmd_dispatch(cmd, width.div_ceil(8), height.div_ceil(8), 1);
            memory_barrier(&self.gpu, cmd);
        }
    }
}

impl Drop for Kernel {
    fn drop(&mut self) {
        unsafe {
            self.gpu.device.destroy_descriptor_pool(self.pool, None);
            self.gpu.device.destroy_pipeline(self.pipeline, None);
            self.gpu.device.destroy_pipeline_layout(self.layout, None);
            self.gpu
                .device
                .destroy_descriptor_set_layout(self.descriptors, None);
        }
    }
}

pub struct CpuInterpolator {
    pub stage_ms: [f32; 4],
    gpu: Arc<Device>,
    previous: Image,
    current: Image,
    output: Image,
    _intermediates: Vec<Image>,
    upload: Buffer,
    download: Buffer,
    motion_params: Buffer,
    interp_params: Buffer,
    luma_params: Buffer,
    kernels: Vec<Kernel>,
    sampler: vk::Sampler,
    flow: [u32; 2],
}

impl CpuInterpolator {
    pub fn new(path: &Path, width: u32, height: u32) -> Result<Self> {
        validate_dimensions(width, height)?;
        let gpu = Device::new(path)?;
        let previous = Image::new(&gpu, width, height, vk::Format::R8G8B8A8_UNORM)?;
        let current = Image::new(&gpu, width, height, vk::Format::R8G8B8A8_UNORM)?;
        let output = Image::new(&gpu, width, height, vk::Format::R32G32B32A32_SFLOAT)?;
        let scale = (160.0 / width as f64).min(90.0 / height as f64).min(1.0);
        let flow = [
            ((width as f64 * scale) as u32 / 4).max(8),
            ((height as f64 * scale) as u32 / 4).max(8),
        ];
        let luma = Image::new(
            &gpu,
            width.div_ceil(4),
            height.div_ceil(4),
            vk::Format::R32G32_SFLOAT,
        )?;
        let coarse = Image::new(
            &gpu,
            (flow[0] / 2).max(4),
            (flow[1] / 2).max(4),
            vk::Format::R32G32B32A32_SFLOAT,
        )?;
        let raw_motion = Image::new(&gpu, flow[0], flow[1], vk::Format::R32G32_SFLOAT)?;
        let raw_occ = Image::new(&gpu, flow[0], flow[1], vk::Format::R32_SFLOAT)?;
        let motion = Image::new(&gpu, flow[0], flow[1], vk::Format::R32G32_SFLOAT)?;
        let occ = Image::new(&gpu, flow[0], flow[1], vk::Format::R32_SFLOAT)?;
        let upload = Buffer::new(&gpu, u64::from(width) * u64::from(height) * 8)?;
        let download = Buffer::new(&gpu, u64::from(width) * u64::from(height) * 16)?;
        let motion_params = Buffer::new(&gpu, 32)?;
        let luma_params = Buffer::new(&gpu, 32)?;
        let interp_params = Buffer::new(&gpu, 16)?;
        let sampler = unsafe {
            gpu.device
                .create_sampler(
                    &vk::SamplerCreateInfo::default()
                        .mag_filter(vk::Filter::LINEAR)
                        .min_filter(vk::Filter::LINEAR)
                        .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .max_lod(0.0),
                    None,
                )
                .map_err(|e| e.to_string())?
        };
        let kernels = (|| {
            Ok(vec![
                Kernel::new(
                    &gpu,
                    include_bytes!(concat!(env!("OUT_DIR"), "/luma.spv")),
                    &[&previous, &current],
                    &[&luma],
                    &luma_params,
                    sampler,
                )?,
                Kernel::new(
                    &gpu,
                    include_bytes!(concat!(env!("OUT_DIR"), "/coarse.spv")),
                    &[&previous, &current, &coarse, &luma],
                    &[&raw_motion, &raw_occ, &coarse],
                    &motion_params,
                    sampler,
                )?,
                Kernel::new(
                    &gpu,
                    include_bytes!(concat!(env!("OUT_DIR"), "/motion.spv")),
                    &[&previous, &current, &coarse, &luma],
                    &[&raw_motion, &raw_occ, &coarse],
                    &motion_params,
                    sampler,
                )?,
                Kernel::new(
                    &gpu,
                    include_bytes!(concat!(env!("OUT_DIR"), "/smooth.spv")),
                    &[&raw_motion, &raw_occ, &previous, &current],
                    &[&motion, &occ],
                    &motion_params,
                    sampler,
                )?,
                Kernel::new(
                    &gpu,
                    include_bytes!(concat!(env!("OUT_DIR"), "/interpolate.spv")),
                    &[&previous, &current, &motion, &occ],
                    &[&output],
                    &interp_params,
                    sampler,
                )?,
            ])
        })();
        let kernels = match kernels {
            Ok(value) => value,
            Err(error) => {
                unsafe {
                    gpu.device.destroy_sampler(sampler, None);
                }
                return Err(error);
            }
        };
        Ok(Self {
            stage_ms: [0.0; 4],
            gpu,
            previous,
            current,
            output,
            _intermediates: vec![luma, coarse, raw_motion, raw_occ, motion, occ],
            upload,
            download,
            motion_params,
            interp_params,
            luma_params,
            kernels,
            sampler,
            flow,
        })
    }

    pub fn device_name(&self) -> &str {
        &self.gpu.name
    }

    pub fn interpolate(&mut self, previous: &[u8], current: &[u8], alpha: f32) -> Result<Vec<u8>> {
        let started = std::time::Instant::now();
        let width = self.output.width;
        let height = self.output.height;
        let count = width as usize * height as usize * 4;
        if previous.len() != count || current.len() != count || !alpha.is_finite() {
            return Err("CPU interpolation input size or phase is invalid".into());
        }
        let mut rgba = Vec::with_capacity(count * 2);
        for pixels in [previous, current] {
            for p in pixels.chunks_exact(4) {
                rgba.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
            }
        }
        self.upload.write(&rgba)?;
        let upload = &self.upload;
        self.gpu.submit(|cmd| unsafe {
            memory_barrier(&self.gpu, cmd);
            self.gpu.device.cmd_copy_buffer_to_image(
                cmd,
                upload.handle,
                self.previous.handle,
                vk::ImageLayout::GENERAL,
                &[self.previous.copy_region()],
            );
            self.gpu.device.cmd_copy_buffer_to_image(
                cmd,
                upload.handle,
                self.current.handle,
                vk::ImageLayout::GENERAL,
                &[self.current.copy_region().buffer_offset(count as u64)],
            );
            memory_barrier(&self.gpu, cmd);
        })?;
        let motion_params =
            |flow: [u32; 2], mode: u32| [width, height, flow[0], flow[1], 8, 2, mode, 2];
        let bytes = |values: &[u32]| {
            values
                .iter()
                .flat_map(|v| v.to_ne_bytes())
                .collect::<Vec<_>>()
        };
        self.luma_params.write(&bytes(&motion_params(
            [width.div_ceil(4), height.div_ceil(4)],
            0,
        )))?;
        self.gpu
            .submit(|cmd| self.kernels[0].dispatch(cmd, width.div_ceil(4), height.div_ceil(4)))?;
        self.stage_ms[0] = started.elapsed().as_secs_f32() * 1000.0;
        let coarse_start = std::time::Instant::now();
        let coarse = [(self.flow[0] / 2).max(4), (self.flow[1] / 2).max(4)];
        self.motion_params
            .write(&bytes(&motion_params(coarse, 0)))?;
        self.gpu
            .submit(|cmd| self.kernels[1].dispatch(cmd, coarse[0], coarse[1]))?;
        self.stage_ms[1] = coarse_start.elapsed().as_secs_f32() * 1000.0;
        let fine_start = std::time::Instant::now();
        self.motion_params
            .write(&bytes(&motion_params(self.flow, 1)))?;
        self.gpu.submit(|cmd| {
            self.kernels[2].dispatch(cmd, self.flow[0], self.flow[1]);
            self.kernels[3].dispatch(cmd, self.flow[0], self.flow[1]);
        })?;
        self.stage_ms[2] = fine_start.elapsed().as_secs_f32() * 1000.0;
        let interp_start = std::time::Instant::now();
        self.interp_params.write(&bytes(&[
            width,
            height,
            alpha.clamp(0.0, 1.0).to_bits(),
            0.5f32.to_bits(),
        ]))?;
        self.gpu.submit(|cmd| unsafe {
            self.kernels[4].dispatch(cmd, width, height);
            self.gpu.device.cmd_copy_image_to_buffer(
                cmd,
                self.output.handle,
                vk::ImageLayout::GENERAL,
                self.download.handle,
                &[self.output.copy_region()],
            );
            memory_barrier(&self.gpu, cmd);
        })?;
        let floats = self.download.read()?;
        let mut bgra = Vec::with_capacity(count);
        for p in floats.chunks_exact(16) {
            let channel = |i| {
                (f32::from_ne_bytes(p[i..i + 4].try_into().unwrap()).clamp(0.0, 1.0) * 255.0)
                    .round() as u8
            };
            bgra.extend_from_slice(&[channel(8), channel(4), channel(0), channel(12)]);
        }
        self.stage_ms[3] = interp_start.elapsed().as_secs_f32() * 1000.0;
        Ok(bgra)
    }
}

impl Drop for CpuInterpolator {
    fn drop(&mut self) {
        unsafe {
            let _ = self.gpu.device.device_wait_idle();
            self.kernels.clear();
            self.gpu.device.destroy_sampler(self.sampler, None);
        }
    }
}

pub struct CpuPresenter {
    gpu: Arc<Device>,
    surface_api: ash::khr::surface::Instance,
    swapchain_api: ash::khr::swapchain::Device,
    surface: vk::SurfaceKHR,
    swapchain: vk::SwapchainKHR,
    images: Vec<vk::Image>,
    initialized: Vec<bool>,
    upload: Buffer,
    extent: vk::Extent2D,
    acquired: vk::Fence,
}

impl CpuPresenter {
    /// The HWND must remain alive until this presenter is dropped.
    pub fn new(path: &Path, hwnd: isize, width: u32, height: u32) -> Result<Self> {
        validate_dimensions(width, height)?;
        let gpu = Device::new(path)?;
        let upload = Buffer::new(&gpu, u64::from(width) * u64::from(height) * 4)?;
        let surface_api = ash::khr::surface::Instance::new(&gpu._entry, &gpu.instance);
        let swapchain_api = ash::khr::swapchain::Device::new(&gpu.instance, &gpu.device);
        let mut out = Self {
            gpu,
            surface_api,
            swapchain_api,
            surface: vk::SurfaceKHR::null(),
            swapchain: vk::SwapchainKHR::null(),
            images: Vec::new(),
            initialized: Vec::new(),
            upload,
            extent: vk::Extent2D { width, height },
            acquired: vk::Fence::null(),
        };
        unsafe {
            let win32 = ash::khr::win32_surface::Instance::new(&out.gpu._entry, &out.gpu.instance);
            out.surface = win32
                .create_win32_surface(
                    &vk::Win32SurfaceCreateInfoKHR::default()
                        .hwnd(hwnd)
                        .hinstance(
                            windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
                                .map_err(|e| e.to_string())?
                                .0 as isize,
                        ),
                    None,
                )
                .map_err(|e| e.to_string())?;
            let family = out
                .gpu
                .instance
                .get_physical_device_queue_family_properties(out.gpu.physical)
                .iter()
                .position(|q| q.queue_flags.contains(vk::QueueFlags::COMPUTE))
                .unwrap() as u32;
            if !out
                .surface_api
                .get_physical_device_surface_support(out.gpu.physical, family, out.surface)
                .map_err(|e| e.to_string())?
            {
                return Err("CPU Vulkan driver cannot present to this window".into());
            }
            let caps = out
                .surface_api
                .get_physical_device_surface_capabilities(out.gpu.physical, out.surface)
                .map_err(|e| e.to_string())?;
            if caps.current_extent != out.extent {
                return Err("CPU Vulkan presentation size changed".into());
            }
            let formats = out
                .surface_api
                .get_physical_device_surface_formats(out.gpu.physical, out.surface)
                .map_err(|e| e.to_string())?;
            let format = formats
                .iter()
                .find(|f| f.format == vk::Format::B8G8R8A8_UNORM)
                .ok_or("CPU driver lacks BGRA presentation")?;
            let composite = [
                vk::CompositeAlphaFlagsKHR::OPAQUE,
                vk::CompositeAlphaFlagsKHR::INHERIT,
            ]
            .into_iter()
            .find(|f| caps.supported_composite_alpha.contains(*f))
            .ok_or("CPU driver lacks opaque presentation")?;
            out.swapchain = out
                .swapchain_api
                .create_swapchain(
                    &vk::SwapchainCreateInfoKHR::default()
                        .surface(out.surface)
                        .min_image_count(caps.min_image_count.max(2).min(
                            if caps.max_image_count == 0 {
                                3
                            } else {
                                caps.max_image_count
                            },
                        ))
                        .image_format(format.format)
                        .image_color_space(format.color_space)
                        .image_extent(out.extent)
                        .image_array_layers(1)
                        .image_usage(vk::ImageUsageFlags::TRANSFER_DST)
                        .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
                        .pre_transform(caps.current_transform)
                        .composite_alpha(composite)
                        .present_mode(vk::PresentModeKHR::FIFO)
                        .clipped(true),
                    None,
                )
                .map_err(|e| e.to_string())?;
            out.images = out
                .swapchain_api
                .get_swapchain_images(out.swapchain)
                .map_err(|e| e.to_string())?;
            out.initialized = vec![false; out.images.len()];
            out.acquired = out
                .gpu
                .device
                .create_fence(&vk::FenceCreateInfo::default(), None)
                .map_err(|e| e.to_string())?;
        }
        Ok(out)
    }

    pub fn present(&mut self, bgra: &[u8]) -> Result<()> {
        if bgra.len() as u64 != self.upload.size {
            return Err("CPU present buffer has incorrect size".into());
        }
        self.upload.write(bgra)?;
        unsafe {
            self.gpu
                .device
                .reset_fences(&[self.acquired])
                .map_err(|e| e.to_string())?;
            let (index, _) = self
                .swapchain_api
                .acquire_next_image(
                    self.swapchain,
                    1_000_000_000,
                    vk::Semaphore::null(),
                    self.acquired,
                )
                .map_err(|e| format!("CPU present acquire: {e}"))?;
            self.gpu
                .device
                .wait_for_fences(&[self.acquired], true, 1_000_000_000)
                .map_err(|e| e.to_string())?;
            let image = self.images[index as usize];
            self.gpu.submit(|cmd| {
                let transition = |old, new| {
                    vk::ImageMemoryBarrier::default()
                        .old_layout(old)
                        .new_layout(new)
                        .image(image)
                        .subresource_range(color_range())
                        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                        .src_access_mask(
                            vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE,
                        )
                        .dst_access_mask(
                            vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE,
                        )
                };
                self.gpu.device.cmd_pipeline_barrier(
                    cmd,
                    vk::PipelineStageFlags::ALL_COMMANDS,
                    vk::PipelineStageFlags::ALL_COMMANDS,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[transition(
                        if self.initialized[index as usize] {
                            vk::ImageLayout::PRESENT_SRC_KHR
                        } else {
                            vk::ImageLayout::UNDEFINED
                        },
                        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    )],
                );
                self.gpu.device.cmd_copy_buffer_to_image(
                    cmd,
                    self.upload.handle,
                    image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &[vk::BufferImageCopy::default()
                        .image_subresource(color_layers())
                        .image_extent(vk::Extent3D {
                            width: self.extent.width,
                            height: self.extent.height,
                            depth: 1,
                        })],
                );
                self.gpu.device.cmd_pipeline_barrier(
                    cmd,
                    vk::PipelineStageFlags::ALL_COMMANDS,
                    vk::PipelineStageFlags::ALL_COMMANDS,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[transition(
                        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                        vk::ImageLayout::PRESENT_SRC_KHR,
                    )],
                );
            })?;
            self.initialized[index as usize] = true;
            self.swapchain_api
                .queue_present(
                    self.gpu.queue,
                    &vk::PresentInfoKHR::default()
                        .swapchains(&[self.swapchain])
                        .image_indices(&[index]),
                )
                .map_err(|e| format!("CPU present: {e}"))?;
            self.gpu
                .device
                .queue_wait_idle(self.gpu.queue)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

impl Drop for CpuPresenter {
    fn drop(&mut self) {
        unsafe {
            let _ = self.gpu.device.device_wait_idle();
            self.gpu.device.destroy_fence(self.acquired, None);
            self.swapchain_api.destroy_swapchain(self.swapchain, None);
            self.surface_api.destroy_surface(self.surface, None);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn rejects_invalid_dimensions_before_loading_the_runtime() {
        let path = std::path::Path::new("not-loaded.dll");
        for (width, height) in [(0, 1), (1, 0), (8193, 1), (1, u32::MAX)] {
            for error in [
                super::CpuInterpolator::new(path, width, height).err(),
                super::CpuPresenter::new(path, 0, width, height).err(),
            ] {
                assert_eq!(error.as_deref(), Some("Invalid CPU frame dimensions"));
            }
        }
    }

    #[test]
    fn rejects_relative_runtime_path() {
        assert!(super::probe_runtime(std::path::Path::new("vk_swiftshader.dll")).is_err());
    }

    #[test]
    #[ignore = "requires FLOWSCALE_CPU_TEST_DRIVER pointing to the pinned SwiftShader DLL"]
    fn software_shader_endpoints_static_translation_and_cut() {
        let path = std::env::var_os("FLOWSCALE_CPU_TEST_DRIVER").expect("CPU test driver");
        let mut pipeline =
            super::CpuInterpolator::new(std::path::Path::new(&path), 128, 96).unwrap();
        assert!(pipeline.device_name().contains("SwiftShader"));
        let static_frame = [37, 93, 151, 255].repeat(128 * 96);
        for alpha in [0.0, 0.5, 1.0] {
            let out = pipeline
                .interpolate(&static_frame, &static_frame, alpha)
                .unwrap();
            assert!(out
                .iter()
                .zip(&static_frame)
                .all(|(a, b)| a.abs_diff(*b) <= 1));
        }
        let frame = |shift: i32| -> Vec<u8> {
            (0..128 * 96)
                .flat_map(|i| {
                    let x = (i % 128 - shift).rem_euclid(128);
                    let y = i / 128;
                    let mut hash = (x as u32).wrapping_mul(1973) ^ (y as u32).wrapping_mul(9277);
                    hash = (hash ^ (hash >> 13)).wrapping_mul(0x85eb_ca6b);
                    let value = 32 + ((hash >> 24) % 192) as u8;
                    [value, value, value, 255]
                })
                .collect()
        };
        let previous = frame(0);
        let current = frame(8);
        let expected = frame(4);
        assert_eq!(
            pipeline.interpolate(&previous, &current, 0.0).unwrap(),
            previous
        );
        assert_eq!(
            pipeline.interpolate(&previous, &current, 1.0).unwrap(),
            current
        );
        let out = pipeline.interpolate(&previous, &current, 0.5).unwrap();
        let motion = &pipeline._intermediates[4];
        let motion_read = super::Buffer::new(
            &pipeline.gpu,
            u64::from(motion.width) * u64::from(motion.height) * 8,
        )
        .unwrap();
        pipeline
            .gpu
            .submit(|cmd| unsafe {
                pipeline.gpu.device.cmd_copy_image_to_buffer(
                    cmd,
                    motion.handle,
                    ash::vk::ImageLayout::GENERAL,
                    motion_read.handle,
                    &[motion.copy_region()],
                );
            })
            .unwrap();
        let data = motion_read.read().unwrap();
        let vectors: Vec<_> = data
            .chunks_exact(8)
            .map(|v| {
                (
                    f32::from_ne_bytes(v[0..4].try_into().unwrap()),
                    f32::from_ne_bytes(v[4..8].try_into().unwrap()),
                )
            })
            .collect();
        eprintln!(
            "CPU motion vector samples {:?}",
            vectors.iter().step_by(37).take(15).collect::<Vec<_>>()
        );
        let mut error = 0u64;
        let mut blend_error = 0u64;
        let mut count = 0;
        for y in 20..76 {
            for x in 20..108 {
                let i = (y * 128 + x) * 4;
                error += out[i].abs_diff(expected[i]) as u64;
                blend_error += ((u16::from(previous[i]) + u16::from(current[i])) / 2)
                    .abs_diff(u16::from(expected[i])) as u64;
                count += 1;
            }
        }
        eprintln!(
            "CPU translated midpoint MAE {:.3}, simple blend MAE {:.3}",
            error as f64 / count as f64,
            blend_error as f64 / count as f64
        );
        assert!(
            error < blend_error,
            "motion interpolation must outperform a crossfade"
        );
        let white = [255u8; 4].repeat(128 * 96);
        let black = [0, 0, 0, 255].repeat(128 * 96);
        let cut = pipeline.interpolate(&black, &white, 0.5).unwrap();
        assert!(cut.chunks_exact(4).all(|p| p[3] == 255));
        assert!(pipeline.interpolate(&black[..10], &white, 0.5).is_err());
        assert!(pipeline.interpolate(&black, &white, f32::NAN).is_err());
    }
}
