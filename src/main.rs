// Flocking boids example with gpu compute update pass
// adapted from https://github.com/austinEng/webgpu-samples/blob/master/src/examples/computeBoids.ts

use nanorand::{Rng, WyRand};
use std::{borrow::Cow, mem};
use wgpu::util::DeviceExt;
use wgpu::Features;

mod framework;


const NUM_PARTICLES: u32 = 1_000_000;
const PARTICLES_PER_GROUP: u32 = 32;

/// Example struct holds references to wgpu resources and frame persistent data
struct Example {
    particle_bind_groups: Vec<wgpu::BindGroup>,
    particle_buffers: Vec<wgpu::Buffer>,
    vertices_buffer: wgpu::Buffer,
    output_staging_buffer: wgpu::Buffer,
    substrate_texture: wgpu::Texture,
    substrate_bind_group: wgpu::BindGroup,
    compute_pipeline: wgpu::ComputePipeline,
    blur_pipeline: wgpu::ComputePipeline,
    render_pipeline: wgpu::RenderPipeline,
    substrate_render_pipeline: wgpu::RenderPipeline,
    work_group_count: u32,
    blur_work_group_count: (u32, u32),
    frame_num: usize,
}

impl crate::framework::Example for Example {
    fn required_features() -> Features {
        // TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES
        Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES
    }
    fn required_limits() -> wgpu::Limits {
        wgpu::Limits::downlevel_defaults()
    }

    fn required_downlevel_capabilities() -> wgpu::DownlevelCapabilities {
        wgpu::DownlevelCapabilities {
            flags: wgpu::DownlevelFlags::COMPUTE_SHADERS,
            ..Default::default()
        }
    }

    /// constructs initial instance of Example struct
    #[allow(clippy::too_many_lines)]
    fn init(
        config: &wgpu::SurfaceConfiguration,
        _adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        _queue: &wgpu::Queue,
    ) -> Self {
        let compute_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!("compute.wgsl"))),
        });
        let draw_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!("draw.wgsl"))),
        });
        let substrate_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!("substrate.wgsl"))),
        });

        // buffer for simulation parameters uniform

        let sim_param_data = [
            0.1,                              // diffusionRate
            22.5 / 360.0 * std::f32::consts::TAU, // sensorAngle
            9.0,                                  // sensorDistance
            45.0 / 360.0 * std::f32::consts::TAU, // rotationAngle
            1.0 / 150.0,                         // stepSize
        ]
        .to_vec();
        let sim_param_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Simulation Parameter Buffer"),
            contents: bytemuck::cast_slice(&sim_param_data),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        // create compute bind layout group and compute pipeline layout

        let compute_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: wgpu::BufferSize::new(
                                (sim_param_data.len() * mem::size_of::<f32>()) as _,
                            ),
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: wgpu::BufferSize::new(u64::from(NUM_PARTICLES * 16)),
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: false },
                            has_dynamic_offset: false,
                            min_binding_size: wgpu::BufferSize::new(u64::from(NUM_PARTICLES * 16)),
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::StorageTexture {
                            access: wgpu::StorageTextureAccess::ReadWrite,
                            format: wgpu::TextureFormat::Rgba16Float,
                            view_dimension: wgpu::TextureViewDimension::D2,
                        },
                        count: None,
                    },
                ],
                label: None,
            });
        let compute_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("compute"),
                bind_group_layouts: &[&compute_bind_group_layout],
                push_constant_ranges: &[],
            });

        // create render pipeline with empty bind group layout

        let render_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("render"),
                bind_group_layouts: &[],
                push_constant_ranges: &[],
            });

        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&render_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &draw_shader,
                entry_point: "main_vs",
                buffers: &[
                    wgpu::VertexBufferLayout {
                        array_stride: 4 * 4,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2],
                    },
                    wgpu::VertexBufferLayout {
                        array_stride: 2 * 4,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![2 => Float32x2],
                    },
                ],
            },
            fragment: Some(wgpu::FragmentState {
                module: &draw_shader,
                entry_point: "main_fs",
                targets: &[Some(config.view_formats[0].into())],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });

        let substrate_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::ReadOnly,
                        format: wgpu::TextureFormat::Rgba16Float,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                }],
                label: None,
            });

        let substrate_render_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("render"),
                bind_group_layouts: &[&substrate_bind_group_layout],
                push_constant_ranges: &[],
            });
        //
        let substrate_render_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: None,
                layout: Some(&substrate_render_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &substrate_shader,
                    entry_point: "substrate_vs",
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &substrate_shader,
                    entry_point: "substrate_fs",
                    targets: &[Some(config.view_formats[0].into())],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
            });

        // create compute pipeline

        let compute_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Compute pipeline"),
            layout: Some(&compute_pipeline_layout),
            module: &compute_shader,
            entry_point: "main",
        });

        let blur_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Blur pipeline"),
            layout: Some(&compute_pipeline_layout),
            module: &compute_shader,
            entry_point: "blur",
        });

        // buffer for the three 2d triangle vertices of each instance

        let vertex_buffer_data = [-0.01f32, -0.02, 0.01, -0.02, 0.00, 0.02].map(|x| x / 10.0);
        let vertices_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Vertex Buffer"),
            contents: bytemuck::bytes_of(&vertex_buffer_data),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });

        // buffer for all particles data of type [(posx,posy,velx,vely),...]

        let mut initial_particle_data = vec![0.0f32; (4 * NUM_PARTICLES) as usize];
        let mut rng = WyRand::new_seed(42);
        let mut unif = || rng.generate::<f32>() * 2f32 - 1f32; // Generate a num (-1, 1)
        for particle_instance_chunk in initial_particle_data.chunks_mut(4) {
            particle_instance_chunk[0] = unif(); // posx
            particle_instance_chunk[1] = unif(); // posy
            particle_instance_chunk[2] = unif() * 0.1; // velx
            particle_instance_chunk[3] = unif() * 0.1; // vely
        }

        // creates two buffers of particle data each of size NUM_PARTICLES
        // the two buffers alternate as dst and src for each frame

        let mut particle_buffers = Vec::<wgpu::Buffer>::new();
        let mut particle_bind_groups = Vec::<wgpu::BindGroup>::new();
        for i in 0..2 {
            particle_buffers.push(
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(&format!("Particle Buffer {i}")),
                    contents: bytemuck::cast_slice(&initial_particle_data),
                    usage: wgpu::BufferUsages::VERTEX
                        | wgpu::BufferUsages::STORAGE
                        | wgpu::BufferUsages::COPY_DST,
                }),
            );
        }

        let substrate_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Substrate Texture"),
            size: wgpu::Extent3d {
                width: config.width,
                height: config.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            view_formats: &[],
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
        });

        let substrate_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &substrate_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(
                    &substrate_texture.create_view(&wgpu::TextureViewDescriptor::default()),
                ),
            }],
            label: None,
        });

        // create two bind groups, one for each buffer as the src
        // where the alternate buffer is used as the dst

        for i in 0..2 {
            particle_bind_groups.push(device.create_bind_group(&wgpu::BindGroupDescriptor {
                layout: &compute_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: sim_param_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: particle_buffers[i].as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: particle_buffers[(i + 1) % 2].as_entire_binding(), // bind to opposite buffer
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(
                            &substrate_texture.create_view(&wgpu::TextureViewDescriptor::default()),
                        ),
                    },
                ],
                label: None,
            }));
        }

        // calculates number of work groups from PARTICLES_PER_GROUP constant
        let work_group_count =
            ((NUM_PARTICLES as f32) / (PARTICLES_PER_GROUP as f32)).ceil() as u32;

        let blur_work_group_count = (
            ((config.width as f32) / 16.0).ceil() as u32,
            ((config.height as f32) / 16.0).ceil() as u32,
        );

        // returns Example struct and No encoder commands
        let staging_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(substrate_texture.width())
                * u64::from(substrate_texture.height())
                * 4
                * 2,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        Example {
            particle_bind_groups,
            particle_buffers,
            vertices_buffer,
            substrate_texture,
            substrate_bind_group,
            compute_pipeline,
            blur_pipeline,
            render_pipeline,
            substrate_render_pipeline,
            output_staging_buffer: staging_buffer,
            work_group_count,
            blur_work_group_count,
            frame_num: 0,
        }
    }

    /// update is called for any WindowEvent not handled by the framework
    fn update(&mut self, _event: winit::event::WindowEvent) {
        //empty
    }

    /// resize is called on WindowEvent::Resized events
    fn resize(
        &mut self,
        _sc_desc: &wgpu::SurfaceConfiguration,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
    ) {
        //empty
    }

    /// render is called each frame, dispatching compute groups proportional
    ///   a TriangleList draw call for all NUM_PARTICLES at 3 vertices each
    fn render(&mut self, view: &wgpu::TextureView, device: &wgpu::Device, queue: &wgpu::Queue) {
        // create render pass descriptor and its color attachments
        let color_attachments = [Some(wgpu::RenderPassColorAttachment {
            view,
            resolve_target: None,
            ops: wgpu::Operations {
                // Not clearing here in order to test wgpu's zero texture initialization on a surface texture.
                // Users should avoid loading uninitialized memory since this can cause additional overhead.
                load: wgpu::LoadOp::Load,
                store: wgpu::StoreOp::Store,
            },
        })];
        let render_pass_descriptor = wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &color_attachments,
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        };

        // get command encoder
        let mut command_encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });

        if self.frame_num < 10 {
            command_encoder.copy_buffer_to_texture(
                wgpu::ImageCopyBuffer {
                    buffer: &self.output_staging_buffer,
                    layout: wgpu::ImageDataLayout {
                        offset: 0,
                        bytes_per_row: Some((4 * self.substrate_texture.width()) * 2),
                        rows_per_image: Some(self.substrate_texture.height()),
                    },
                },
                wgpu::ImageCopyTexture {
                    texture: &self.substrate_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: self.substrate_texture.width(),
                    height: self.substrate_texture.height(),
                    depth_or_array_layers: 1,
                },
            );
        }

        command_encoder.push_debug_group("compute movement");
        {
            // compute pass
            let mut cpass = command_encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: None,
                timestamp_writes: None,
            });
            cpass.set_pipeline(&self.compute_pipeline);
            cpass.set_bind_group(0, &self.particle_bind_groups[self.frame_num % 2], &[]);
            cpass.dispatch_workgroups(self.work_group_count, 1, 1);
        }
        {

            let mut bpass = command_encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: None,
                timestamp_writes: None,
            });
            bpass.set_pipeline(&self.blur_pipeline);
            bpass.set_bind_group(0, &self.particle_bind_groups[self.frame_num % 2], &[]);
            bpass.dispatch_workgroups(self.blur_work_group_count.0, self.blur_work_group_count.1, 1);
        }
        command_encoder.pop_debug_group();

        command_encoder.push_debug_group("render substrate");
        {
            let mut srpass = command_encoder.begin_render_pass(&render_pass_descriptor);
            srpass.set_pipeline(&self.substrate_render_pipeline);
            srpass.set_bind_group(0, &self.substrate_bind_group, &[]);
            srpass.draw(0..3, 0..2); // TODO
        }
        command_encoder.pop_debug_group();

        command_encoder.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture: &self.substrate_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &self.output_staging_buffer,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    // This needs to be padded to 256.
                    bytes_per_row: Some((4 * self.substrate_texture.width()) * 2),
                    rows_per_image: Some(self.substrate_texture.height()),
                },
            },
            wgpu::Extent3d {
                width: self.substrate_texture.width(),
                height: self.substrate_texture.height(),
                depth_or_array_layers: 1,
            },
        );

        command_encoder.push_debug_group("render agents");
        {
            // render pass
            let mut rpass = command_encoder.begin_render_pass(&render_pass_descriptor);
            rpass.set_pipeline(&self.render_pipeline);
            // render dst particles
            rpass.set_vertex_buffer(0, self.particle_buffers[(self.frame_num + 1) % 2].slice(..));
            // the three instance-local vertices
            rpass.set_vertex_buffer(1, self.vertices_buffer.slice(..));
            rpass.draw(0..3, 0..NUM_PARTICLES);
        }
        command_encoder.pop_debug_group();

        // update frame count
        self.frame_num += 1;

        // done
        queue.submit(Some(command_encoder.finish()));
    }
}

/// run example
pub fn main() {
    crate::framework::run::<Example>("Physarum");
}
