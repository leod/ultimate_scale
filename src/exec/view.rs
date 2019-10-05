use std::collections::BTreeSet;
use std::time::Duration;

use log::info;

use nalgebra as na;

use glutin::{VirtualKeyCode, WindowEvent};

use crate::exec::{Exec, WindState};
use crate::machine::grid::{Dir3, Point3};
use crate::machine::{grid, BlipKind, BlockIndex, Machine};
use crate::render::{self, Camera, EditCameraView, RenderLists};
use crate::util::intersection::{ray_aabb_intersection, Ray, AABB};
use crate::util::timer::Timer;

#[derive(Debug, Clone)]
pub struct Config {
    pub pause_resume_key: VirtualKeyCode,
    pub stop_key: VirtualKeyCode,
    pub frame_key: VirtualKeyCode,
    pub default_ticks_per_sec: f32,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            pause_resume_key: VirtualKeyCode::Space,
            stop_key: VirtualKeyCode::Escape,
            frame_key: VirtualKeyCode::F,
            default_ticks_per_sec: 0.5,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Status {
    Playing,
    Paused,
    Stopped,
}

pub struct ExecView {
    config: Config,
    exec: Exec,
    tick_timer: Timer,
    status: Status,

    mouse_window_pos: na::Point2<f32>,
    mouse_grid_pos: Option<grid::Point3>,
}

impl ExecView {
    pub fn new(config: &Config, machine: Machine) -> ExecView {
        ExecView {
            config: config.clone(),
            exec: Exec::new(machine),
            tick_timer: Timer::from_hz(config.default_ticks_per_sec),
            status: Status::Playing,
            mouse_window_pos: na::Point2::origin(),
            mouse_grid_pos: None,
        }
    }

    pub fn status(&self) -> Status {
        self.status
    }

    pub fn cur_tick_time(&self) -> f32 {
        self.exec.cur_tick as f32 + self.tick_timer.progress()
    }

    pub fn update(&mut self, dt: Duration, camera: &Camera, edit_camera_view: &EditCameraView) {
        self.update_mouse_grid_pos(camera, edit_camera_view);

        match self.status {
            Status::Playing => {
                self.tick_timer += dt;

                // TODO: Run multiple ticks on lag spikes? If so, with some
                //       upper limit?
                if self.tick_timer.trigger_reset() {
                    self.exec.update();
                }
            }
            Status::Paused => (),
            Status::Stopped => {
                // Game::update will return to editor
            }
        }
    }

    pub fn on_event(&mut self, event: &WindowEvent) {
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                self.mouse_window_pos = na::Point2::new(position.x as f32, position.y as f32);
            }
            WindowEvent::KeyboardInput { input, .. } => self.on_keyboard_input(*input),
            WindowEvent::MouseInput {
                state,
                button,
                modifiers,
                ..
            } => self.on_mouse_input(*state, *button, *modifiers),
            _ => (),
        }
    }

    fn on_keyboard_input(&mut self, input: glutin::KeyboardInput) {
        if input.state == glutin::ElementState::Pressed {
            if let Some(keycode) = input.virtual_keycode {
                self.on_key_press(keycode);
            }
        }
    }

    fn on_key_press(&mut self, keycode: VirtualKeyCode) {
        if keycode == self.config.pause_resume_key {
            match self.status {
                Status::Playing => {
                    info!("Pausing exec");
                    self.status = Status::Paused;
                }
                Status::Paused => {
                    info!("Resuming exec");
                    self.status = Status::Playing;
                }
                Status::Stopped => {
                    // Should happen only if pause is pressed after stop in the
                    // same frame -- just ignore.
                }
            }
        } else if keycode == self.config.stop_key {
            self.status = Status::Stopped;
        } else if keycode == self.config.frame_key {
            info!("Running single frame");
            self.exec.update();
            self.tick_timer.reset();
        }
    }

    fn on_mouse_input(
        &mut self,
        state: glutin::ElementState,
        button: glutin::MouseButton,
        _modifiers: glutin::ModifiersState,
    ) {
        match button {
            glutin::MouseButton::Left if state == glutin::ElementState::Pressed => {
                if let Some(mouse_grid_pos) = self.mouse_grid_pos {
                    Exec::try_spawn_blip(
                        false,
                        BlipKind::A,
                        &mouse_grid_pos,
                        &self.exec.machine.blocks.indices,
                        &mut self.exec.blip_state,
                        &mut self.exec.blips,
                    );
                }
            }
            glutin::MouseButton::Right if state == glutin::ElementState::Pressed => {
                if let Some(mouse_grid_pos) = self.mouse_grid_pos {
                    Exec::try_spawn_blip(
                        false,
                        BlipKind::B,
                        &mouse_grid_pos,
                        &self.exec.machine.blocks.indices,
                        &mut self.exec.blip_state,
                        &mut self.exec.blips,
                    );
                }
            }
            _ => (),
        }
    }

    pub fn render(&mut self, out: &mut RenderLists) {
        render::machine::render_machine(&self.exec.machine(), self.cur_tick_time(), out);

        self.render_blocks(out);
        self.render_blips(out);

        if let Some(mouse_grid_pos) = self.mouse_grid_pos {
            assert!(self.exec.machine().is_valid_pos(&mouse_grid_pos));

            let mouse_grid_pos_float: na::Point3<f32> = na::convert(mouse_grid_pos);

            render::machine::render_cuboid_wireframe(
                &render::machine::Cuboid {
                    center: mouse_grid_pos_float + na::Vector3::new(0.5, 0.5, 0.51),
                    size: na::Vector3::new(1.0, 1.0, 1.0),
                },
                0.015,
                &na::Vector4::new(0.9, 0.9, 0.9, 1.0),
                &mut out.solid,
            );
        }
    }

    fn wind_dir_pairs(
        &self,
        wind_state: &[WindState],
        block_index: BlockIndex,
        block_pos: &Point3,
    ) -> BTreeSet<(Dir3, Option<Dir3>)> {
        // In which directions are our neighbors getting flow from us?
        let mut out_dirs: Vec<_> = self
            .exec
            .machine()
            .iter_neighbors(block_pos)
            .filter(|(dir, neighbor_index)| wind_state[*neighbor_index].wind_in(dir.invert()))
            .map(|(dir, _)| dir)
            .collect();

        // From which directions are we getting flow from a neighbor?
        let in_dirs = Dir3::ALL
            .iter()
            .filter(|dir| wind_state[block_index].wind_in(**dir));

        let mut dir_pairs = BTreeSet::new();

        for &in_dir in in_dirs {
            if out_dirs.is_empty() {
                // There is no wind flowing out of this block
                dir_pairs.insert((in_dir, None));
            } else {
                for &out_dir in &out_dirs {
                    dir_pairs.insert((in_dir, Some(out_dir)));
                }
            }
        }

        dir_pairs
    }

    fn render_blocks(&self, out: &mut RenderLists) {
        let wind_state = self.exec.wind_state();
        let old_wind_state = self.exec.old_wind_state();

        for (block_index, (block_pos, _placed_block)) in self.exec.machine().blocks.data.iter() {
            let block_wind_state = &wind_state[block_index];

            let dir_pairs = self.wind_dir_pairs(wind_state, block_index, block_pos);

            for &(in_dir, out_dir) in &dir_pairs {
                let center = render::machine::block_center(block_pos);

                let in_vector: na::Vector3<f32> = na::convert(in_dir.to_vector());

                // If the out_dir is None, there is no outgoing flow, so just
                // draw the line to the block center
                let out_vector: na::Vector3<f32> = out_dir
                    .map_or(na::Vector3::zeros(), |out_dir| {
                        na::convert(out_dir.to_vector())
                    });

                let in_pos = center + in_vector / 2.0;
                let out_pos = center + out_vector / 2.0;

                render::machine::render_arrow(
                    &render::machine::Line {
                        start: in_pos,
                        end: out_pos,
                        thickness: 0.05,
                        color: na::Vector4::new(1.0, 0.0, 0.0, 1.0),
                    },
                    0.0,
                    &mut out.solid,
                );
            }
        }
    }

    fn render_blips(&self, out: &mut RenderLists) {
        for (_index, blip) in self.exec.blips().iter() {
            /*if blip.old_pos.is_none() {
                // Workaround for the fact that we use old blip positions but
                // render new machine state
                continue;
            }*/

            let center = render::machine::block_center(&blip.pos);

            let pos = if let Some(old_pos) = blip.old_pos {
                let old_center = render::machine::block_center(&blip.old_pos.unwrap());
                old_center + self.tick_timer.progress() * (center - old_center)
            } else {
                center
            };

            let size = if blip.old_pos.is_none() {
                // Animate spawning the blip
                if self.tick_timer.progress() < 0.75 {
                    0.0
                } else {
                    (self.tick_timer.progress() - 0.75) * 4.0
                }
            } else {
                1.0
            } * 0.3;

            let transform =
                na::Matrix4::new_translation(&pos.coords) * na::Matrix4::new_scaling(size);
            let color = render::machine::blip_color(blip.kind);
            let instance = render::Instance {
                object: render::Object::Cube,
                params: render::InstanceParams {
                    color: na::Vector4::new(color.x, color.y, color.z, 1.0),
                    transform,
                    ..Default::default()
                },
            };

            out.solid.add_instance(&instance);
            out.solid_shadow.add_instance(&instance);

            out.lights.push(render::Light {
                position: pos,
                attenuation: na::Vector3::new(0.0, 0.0, 100.0),
                color: na::Vector3::new(0.2, 10.0, 0.5),
                radius: 10.0,
            });
        }
    }

    fn update_mouse_grid_pos(&mut self, camera: &Camera, edit_camera_view: &EditCameraView) {
        let p = self.mouse_window_pos;
        let p_near = camera.unproject(&na::Point3::new(p.x, p.y, -1.0));
        let p_far = camera.unproject(&na::Point3::new(p.x, p.y, 1.0));

        let ray = Ray {
            origin: edit_camera_view.eye(),
            velocity: p_far - p_near,
        };

        let mut closest_block = None;

        for (_block_index, (block_pos, _placed_block)) in self.exec.machine().iter_blocks() {
            let center = render::machine::block_center(&block_pos);

            let aabb = AABB {
                min: center - na::Vector3::new(0.5, 0.5, 0.5),
                max: center + na::Vector3::new(0.5, 0.5, 0.5),
            };

            if let Some(distance) = ray_aabb_intersection(&ray, &aabb) {
                closest_block = Some(closest_block.map_or(
                    (block_pos, distance),
                    |(closest_pos, closest_distance)| {
                        if distance < closest_distance {
                            (block_pos, distance)
                        } else {
                            (closest_pos, closest_distance)
                        }
                    },
                ));
            }
        }

        self.mouse_grid_pos = closest_block.map(|(pos, _distance)| *pos);
    }
}
