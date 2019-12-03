pub mod deferred;
pub mod glow;
pub mod render_pass;
pub mod shaders;
pub mod shadow;

pub use render_pass::{CompositionPassComponent, RenderPass, ScenePassComponent};

use log::info;
use coarse_prof::profile;

use glium::{uniform, Surface};

use crate::config::ViewConfig;
use crate::render::fxaa::{self, FXAA};
use crate::render::shader::{ToUniforms, ToVertex, UniformInput};
use crate::render::{
    self, object, scene, screen_quad, shader, Context, DrawError, Instancing, Light, RenderList,
    RenderLists, Resources, ScreenQuad,
};

use deferred::DeferredShading;
use glow::Glow;
use shadow::ShadowMapping;

#[derive(Debug, Clone)]
pub struct Config {
    pub shadow_mapping: Option<shadow::Config>,
    pub deferred_shading: Option<deferred::Config>,
    pub glow: Option<glow::Config>,
    pub hdr: Option<f32>,
    pub gamma_correction: Option<f32>,
    pub fxaa: Option<fxaa::Config>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            shadow_mapping: Some(Default::default()),
            deferred_shading: Some(Default::default()),
            glow: Some(Default::default()),
            hdr: None,
            gamma_correction: Some(2.2),
            fxaa: Some(Default::default()),
        }
    }
}

struct Components {
    shadow_mapping: Option<ShadowMapping>,
    deferred_shading: Option<DeferredShading>,
    glow: Option<Glow>,
}

#[derive(Debug, Clone)]
struct ScenePassSetup {
    shadow: bool,
    glow: bool,
}

struct ScenePass<I: ToVertex, V> {
    setup: ScenePassSetup,

    /// Currently just used as a phantom.
    #[allow(dead_code)]
    shader_core: shader::Core<Context, I, V>,

    program: glium::Program,

    instancing: Instancing<I>,
}

impl Components {
    fn create<F: glium::backend::Facade>(
        facade: &F,
        config: &Config,
        view_config: &ViewConfig,
    ) -> Result<Self, CreationError> {
        let shadow_mapping = config
            .shadow_mapping
            .as_ref()
            .map(|config| ShadowMapping::create(facade, config))
            .transpose()
            .map_err(CreationError::ShadowMapping)?;

        let deferred_shading = config
            .deferred_shading
            .as_ref()
            .map(|config| {
                DeferredShading::create(
                    facade,
                    &config,
                    shadow_mapping.is_some(),
                    view_config.window_size,
                )
            })
            .transpose()
            .map_err(CreationError::DeferredShading)?;

        let glow = config
            .glow
            .as_ref()
            .map(|config| Glow::create(facade, config, view_config.window_size))
            .transpose()
            .map_err(CreationError::Glow)?;

        Ok(Self {
            shadow_mapping,
            deferred_shading,
            glow,
        })
    }

    fn create_scene_pass<F, I: ToVertex, V>(
        &self,
        facade: &F,
        setup: ScenePassSetup,
        mut shader_core: shader::Core<Context, I, V>,
    ) -> Result<ScenePass<I, V>, render::CreationError>
    where
        F: glium::backend::Facade,
        I: UniformInput + Clone,
        V: glium::vertex::Vertex,
    {
        info!(
            "Creating scene pass for I={}, V={}",
            std::any::type_name::<I>(),
            std::any::type_name::<V>(),
        );

        if let Some(glow) = self.glow.as_ref() {
            if setup.glow {
                shader_core = ScenePassComponent::core_transform(glow, shader_core);
            } else {
                // Whoopsie there goes the abstraction, heh. All good though.
                shader_core = glow::shaders::no_glow_map_core_transform(shader_core);
            }
        }

        if let Some(shadow_mapping) = self.shadow_mapping.as_ref() {
            if setup.shadow {
                shader_core = ScenePassComponent::core_transform(shadow_mapping, shader_core);
            }
        }

        if let Some(deferred_shading) = self.deferred_shading.as_ref() {
            shader_core = ScenePassComponent::core_transform(deferred_shading, shader_core);
        } else {
            shader_core = shaders::diffuse_scene_core_transform(shader_core);
        }

        let program = shader_core.build_program(facade, shader::InstancingMode::Vertex)?;

        let instancing = Instancing::create(facade)?;

        Ok(ScenePass {
            setup,
            shader_core,
            program,
            instancing,
        })
    }

    fn composition_core(&self, config: &Config) -> shader::Core<(), (), screen_quad::Vertex> {
        let mut shader_core = shaders::composition_core();

        if let Some(deferred_shading) = self.deferred_shading.as_ref() {
            shader_core = CompositionPassComponent::core_transform(deferred_shading, shader_core);
        }

        if let Some(glow) = self.glow.as_ref() {
            shader_core = CompositionPassComponent::core_transform(glow, shader_core);
        }

        if let Some(_) = config.hdr {
            // TODO: Use factor
            shader_core = shaders::hdr_composition_core_transform(shader_core);
        }

        if let Some(gamma) = config.gamma_correction {
            shader_core = shaders::gamma_correction_composition_core_transform(shader_core, gamma);
        }

        shader_core
    }

    fn clear_buffers<F: glium::backend::Facade>(&self, facade: &F) -> Result<(), DrawError> {
        self.shadow_mapping
            .as_ref()
            .map(|c| c.clear_buffers(facade))
            .transpose()?;
        self.deferred_shading
            .as_ref()
            .map(|c| c.clear_buffers(facade))
            .transpose()?;
        self.glow
            .as_ref()
            .map(|c| c.clear_buffers(facade))
            .transpose()?;

        Ok(())
    }

    fn scene_output_textures(
        &self,
        setup: &ScenePassSetup,
    ) -> Vec<(&'static str, &glium::texture::Texture2d)> {
        let mut textures = Vec::new();

        textures.extend(
            self.deferred_shading
                .as_ref()
                .map_or(Vec::new(), ScenePassComponent::output_textures),
        );

        if setup.glow {
            textures.extend(
                self.glow
                    .as_ref()
                    .map_or(Vec::new(), ScenePassComponent::output_textures),
            );
        }

        textures
    }

    fn scene_pass_to_surface<I, V, S>(
        &self,
        resources: &Resources,
        context: &Context,
        pass: &ScenePass<I, V>,
        _render_list: &RenderList<I>,
        target: &mut S,
    ) -> Result<(), DrawError>
    where
        I: ToUniforms + ToVertex,
        V: glium::vertex::Vertex,
        S: glium::Surface,
    {
        let params = glium::DrawParameters {
            backface_culling: glium::draw_parameters::BackfaceCullingMode::CullClockwise,
            depth: glium::Depth {
                test: glium::DepthTest::IfLessOrEqual,
                write: true,
                ..Default::default()
            },
            line_width: Some(2.0),
            ..Default::default()
        };

        let uniforms = (
            context,
            self.shadow_mapping
                .as_ref()
                .map(|c| c.scene_pass_uniforms(context)),
        );

        pass.instancing.draw(
            resources,
            &pass.program,
            &uniforms.to_uniforms(),
            &params,
            target,
        )?;

        Ok(())
    }

    fn scene_pass<F, I, V>(
        &self,
        facade: &F,
        resources: &Resources,
        context: &Context,
        pass: &ScenePass<I, V>,
        render_list: &RenderList<I>,
        color_texture: &glium::texture::Texture2d,
        depth_texture: &glium::texture::DepthTexture2d,
    ) -> Result<(), DrawError>
    where
        F: glium::backend::Facade,
        I: ToUniforms + ToVertex,
        V: glium::vertex::Vertex,
    {
        let mut output_textures = self.scene_output_textures(&pass.setup);
        output_textures.push((shader::defs::F_COLOR, color_texture));

        let mut framebuffer = glium::framebuffer::MultiOutputFrameBuffer::with_depth_buffer(
            facade,
            output_textures.into_iter(),
            depth_texture,
        )?;

        self.scene_pass_to_surface(resources, context, pass, render_list, &mut framebuffer)
    }
}

pub struct Pipeline {
    components: Components,

    scene_pass_solid: ScenePass<scene::model::Params, object::Vertex>,
    scene_pass_solid_glow: ScenePass<scene::model::Params, object::Vertex>,
    scene_pass_wind: ScenePass<scene::wind::Params, object::Vertex>,

    scene_pass_plain: ScenePass<scene::model::Params, object::Vertex>,

    scene_color_texture: glium::texture::Texture2d,
    scene_depth_texture: glium::texture::DepthTexture2d,

    composition_program: glium::Program,
    composition_texture: glium::texture::Texture2d,

    fxaa: Option<FXAA>,
    copy_texture_program: glium::Program,

    screen_quad: ScreenQuad,
}

impl Pipeline {
    pub fn create<F: glium::backend::Facade>(
        facade: &F,
        config: &Config,
        view_config: &ViewConfig,
    ) -> Result<Pipeline, CreationError> {
        let components = Components::create(facade, config, view_config)?;

        let scene_pass_solid = components.create_scene_pass(
            facade,
            ScenePassSetup {
                shadow: true,
                glow: false,
            },
            scene::model::scene_core(),
        )?;
        let scene_pass_solid_glow = components.create_scene_pass(
            facade,
            ScenePassSetup {
                shadow: true,
                glow: true,
            },
            scene::model::scene_core(),
        )?;
        let scene_pass_wind = components.create_scene_pass(
            facade,
            ScenePassSetup {
                shadow: false,
                glow: true,
            },
            scene::wind::scene_core(),
        )?;

        let plain_core = scene::model::scene_core();
        let plain_program = plain_core
            .build_program(facade, shader::InstancingMode::Vertex)
            .map_err(render::CreationError::from)?;
        let plain_instancing = Instancing::create(facade)?;
        let scene_pass_plain = ScenePass {
            setup: ScenePassSetup {
                shadow: false,
                glow: false,
            },
            shader_core: plain_core,
            program: plain_program,
            instancing: plain_instancing,
        };

        let rounded_size: (u32, u32) = view_config.window_size.into();
        let scene_color_texture = Self::create_color_texture(facade, rounded_size)?;
        let scene_depth_texture = Self::create_depth_texture(facade, rounded_size)?;

        let composition_core = components.composition_core(config);
        let composition_program = composition_core
            .build_program(facade, shader::InstancingMode::Uniforms)
            .map_err(render::CreationError::from)?;
        let composition_texture = Self::create_color_texture(facade, rounded_size)?;

        let fxaa = config
            .fxaa
            .as_ref()
            .map(|config| fxaa::FXAA::create(facade, config))
            .transpose()
            .map_err(CreationError::FXAA)?;
        let copy_texture_program = shaders::composition_core()
            .build_program(facade, shader::InstancingMode::Uniforms)
            .map_err(render::CreationError::from)?;

        info!("Creating screen quad");
        let screen_quad = ScreenQuad::create(facade)?;

        info!("Pipeline initialized");

        Ok(Pipeline {
            components,
            scene_pass_solid,
            scene_pass_solid_glow,
            scene_pass_plain,
            scene_pass_wind,
            scene_color_texture,
            scene_depth_texture,
            composition_program,
            composition_texture,
            fxaa,
            copy_texture_program,
            screen_quad,
        })
    }

    pub fn draw_frame<F: glium::backend::Facade, S: glium::Surface>(
        &mut self,
        facade: &F,
        resources: &Resources,
        context: &Context,
        render_lists: &RenderLists,
        target: &mut S,
    ) -> Result<(), DrawError> {
        profile!("pipeline");

        // Send instance data to GPU
        {
            profile!("send_data");

            self.scene_pass_solid
                .instancing
                .update(facade, &render_lists.solid.instances)?;
            self.scene_pass_solid_glow
                .instancing
                .update(facade, &render_lists.solid_glow.instances)?;
            self.scene_pass_plain
                .instancing
                .update(facade, &render_lists.plain.instances)?;
            self.scene_pass_wind
                .instancing
                .update(facade, &render_lists.wind.instances)?;
        }

        // Clear buffers
        {
            profile!("clear");

            let mut framebuffer = glium::framebuffer::SimpleFrameBuffer::with_depth_buffer(
                facade,
                &self.scene_color_texture,
                &self.scene_depth_texture,
            )?;
            framebuffer.clear_color_and_depth((0.0, 0.0, 0.0, 1.0), 1.0);

            self.components.clear_buffers(facade)?;
        }

        // Create shadow map from the main light's point of view
        if let Some(shadow_mapping) = self.components.shadow_mapping.as_ref() {
            profile!("shadow_pass");

            shadow_mapping.shadow_pass(
                facade,
                resources,
                context,
                &self.scene_pass_solid.instancing,
                &self.scene_pass_solid_glow.instancing,
            )?;
        }

        // Render scene into buffers
        {
            profile!("scene_pass");

            self.components.scene_pass(
                facade,
                resources,
                context,
                &self.scene_pass_solid,
                &render_lists.solid,
                &self.scene_color_texture,
                &self.scene_depth_texture,
            )?;
            self.components.scene_pass(
                facade,
                resources,
                context,
                &self.scene_pass_solid_glow,
                &render_lists.solid_glow,
                &self.scene_color_texture,
                &self.scene_depth_texture,
            )?;
            self.components.scene_pass(
                facade,
                resources,
                context,
                &self.scene_pass_wind,
                &render_lists.wind,
                &self.scene_color_texture,
                &self.scene_depth_texture,
            )?;
        }

        // Render light sources into a buffer
        if let Some(deferred_shading) = self.components.deferred_shading.as_mut() {
            profile!("light_pass");

            deferred_shading.light_pass(
                facade,
                resources,
                &context.camera,
                &render_lists.lights,
            )?;
        }

        // Blur the glow texture
        if let Some(glow) = self.components.glow.as_ref() {
            profile!("blur_glow_pass");

            glow.blur_pass(facade)?;
        }

        // Combine buffers
        {
            profile!("composition_pass");

            let mut target_buffer =
                glium::framebuffer::SimpleFrameBuffer::new(facade, &self.composition_texture)?;

            let color_uniform = uniform! {
                color_texture: &self.scene_color_texture,
            };
            let deferred_shading_uniforms = self
                .components
                .deferred_shading
                .as_ref()
                .map(|c| c.composition_pass_uniforms());
            let glow_uniforms = self
                .components
                .glow
                .as_ref()
                .map(|c| c.composition_pass_uniforms());

            let uniforms = (&color_uniform, &deferred_shading_uniforms, &glow_uniforms);

            target_buffer.draw(
                &self.screen_quad.vertex_buffer,
                &self.screen_quad.index_buffer,
                &self.composition_program,
                &uniforms.to_uniforms(),
                &Default::default(),
            )?;
        }

        // Draw plain stuff on top
        {
            profile!("plain");

            let mut framebuffer = glium::framebuffer::SimpleFrameBuffer::with_depth_buffer(
                facade,
                &self.composition_texture,
                &self.scene_depth_texture,
            )?;

            self.components.scene_pass_to_surface(
                resources,
                context,
                &self.scene_pass_plain,
                &render_lists.plain,
                &mut framebuffer,
            )?;
        }

        // Postprocessing
        if let Some(fxaa) = self.fxaa.as_ref() {
            profile!("fxaa");

            fxaa.draw(&self.composition_texture, target)?;
        } else {
            profile!("copy_to_target");

            target.draw(
                &self.screen_quad.vertex_buffer,
                &self.screen_quad.index_buffer,
                &self.copy_texture_program,
                &uniform! {
                    color_texture: &self.composition_texture,
                },
                &Default::default(),
            )?;
        }

        Ok(())
    }

    pub fn on_window_resize<F: glium::backend::Facade>(
        &mut self,
        facade: &F,
        new_window_size: glium::glutin::dpi::LogicalSize,
    ) -> Result<(), CreationError> {
        if let Some(deferred_shading) = self.components.deferred_shading.as_mut() {
            deferred_shading
                .on_window_resize(facade, new_window_size)
                .map_err(CreationError::DeferredShading)?;
        }

        if let Some(glow) = self.components.glow.as_mut() {
            glow.on_window_resize(facade, new_window_size)
                .map_err(CreationError::Glow)?;
        }

        let rounded_size: (u32, u32) = new_window_size.into();
        self.scene_color_texture = Self::create_color_texture(facade, rounded_size)?;
        self.scene_depth_texture = Self::create_depth_texture(facade, rounded_size)?;

        self.composition_texture = Self::create_color_texture(facade, rounded_size)?;

        Ok(())
    }

    fn create_color_texture<F: glium::backend::Facade>(
        facade: &F,
        size: (u32, u32),
    ) -> Result<glium::texture::Texture2d, CreationError> {
        Ok(glium::texture::Texture2d::empty_with_format(
            facade,
            glium::texture::UncompressedFloatFormat::F32F32F32F32,
            glium::texture::MipmapsOption::NoMipmap,
            size.0,
            size.1,
        )
        .map_err(render::CreationError::from)?)
    }

    fn create_depth_texture<F: glium::backend::Facade>(
        facade: &F,
        size: (u32, u32),
    ) -> Result<glium::texture::DepthTexture2d, render::CreationError> {
        Ok(glium::texture::DepthTexture2d::empty_with_format(
            facade,
            glium::texture::DepthFormat::F32,
            glium::texture::MipmapsOption::NoMipmap,
            size.0,
            size.1,
        )
        .map_err(render::CreationError::from)?)
    }
}

#[derive(Debug)]
pub enum CreationError {
    ShadowMapping(shadow::CreationError),
    DeferredShading(deferred::CreationError),
    Glow(glow::CreationError),
    FXAA(fxaa::CreationError),
    CreationError(render::CreationError),
}

impl From<render::CreationError> for CreationError {
    fn from(err: render::CreationError) -> CreationError {
        CreationError::CreationError(err)
    }
}
