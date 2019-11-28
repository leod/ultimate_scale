use glium::glutin;

use crate::edit;
use crate::edit_camera_view;
use crate::exec;
use crate::render;

#[derive(Debug, Clone)]
pub struct ViewConfig {
    pub window_size: glutin::dpi::LogicalSize,
    pub fov_degrees: f64,
}

impl Default for ViewConfig {
    fn default() -> ViewConfig {
        ViewConfig {
            window_size: glutin::dpi::LogicalSize::new(1280.0, 720.0),
            fov_degrees: 60.0,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Config {
    pub camera: edit_camera_view::Config,
    pub view: ViewConfig,
    pub render_pipeline: render::pipeline::Config,
    pub editor: edit::Config,
    pub exec: exec::view::Config,
    pub play: exec::play::Config,
}
