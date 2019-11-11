pub mod grid;
pub mod level;

use serde::{Deserialize, Serialize};

use crate::util::vec_option::VecOption;

use grid::{Axis3, Dir3, Grid3, Point3, Sign, Vector3};
use level::Level;

#[derive(PartialEq, Eq, Copy, Clone, Debug, Serialize, Deserialize)]
pub enum BlipKind {
    A,
    B,
    C,
}

impl Default for BlipKind {
    fn default() -> BlipKind {
        BlipKind::A
    }
}

impl BlipKind {
    pub fn name(self) -> &'static str {
        match self {
            BlipKind::A => "a",
            BlipKind::B => "b",
            BlipKind::C => "c",
        }
    }

    pub fn next(self) -> BlipKind {
        match self {
            BlipKind::A => BlipKind::B,
            BlipKind::B => BlipKind::C,
            BlipKind::C => BlipKind::A,
        }
    }
}

pub type TickNum = usize;

/// Definition of a block in the machine.
///
/// This definition is somewhat "dirty" in that it also contains state that is
/// only needed at execution time -- e.g. the `activated` fields in some of the
/// blocks. Consider this an artifact of us not using an ECS.
///
/// Note also that most of the `Block` variants are not rotated in space. For
/// example, in the definition of `Block::BlipWindSource`, the input direction
/// is hardcoded as `Dir3::Y_NEG`. On a higher level, `PlacedBlock` allows
/// rotating a `Block` in the X-Y plane.
#[derive(PartialEq, Eq, Clone, Debug, Serialize, Deserialize)]
pub enum Block {
    Pipe(Dir3, Dir3),
    PipeSplitXY {
        open_move_hole_y: Sign,
    },
    PipeMergeXY,
    FunnelXY,
    WindSource,
    BlipSpawn {
        kind: BlipKind,
        num_spawns: Option<usize>,
        #[serde(default)]
        activated: Option<TickNum>,
    },
    BlipDuplicator {
        #[serde(default)]
        kind: Option<BlipKind>,
        activated: Option<BlipKind>,
    },
    BlipWindSource {
        activated: bool,
    },
    Solid,
    Input {
        index: usize,
        inputs: Vec<Option<level::Input>>,
        activated: Option<level::Input>,
    },
    Output {
        index: usize,
        expected_next_kind: Option<BlipKind>,
    },
}

impl Block {
    pub fn name(&self) -> String {
        match self {
            Block::Pipe(a, b) if a.0 != Axis3::Z && a.0 == b.0 => "Pipe straight".to_string(),
            Block::Pipe(a, b) if a.0 != Axis3::Z && b.0 != Axis3::Z && a.0 != b.0 => {
                "Pipe curve".to_string()
            }
            Block::Pipe(a, b) if a.0 == Axis3::Z && a.0 == b.0 => "Pipe up/down".to_string(),
            Block::Pipe(a, b) if (*a == Dir3::Z_NEG || *b == Dir3::Z_NEG) && a.0 != b.0 => {
                "Pipe curve down".to_string()
            }
            Block::Pipe(a, b) if (*a == Dir3::Z_POS || *b == Dir3::Z_POS) && a.0 != b.0 => {
                "Pipe curve up".to_string()
            }
            Block::Pipe(_, _) => "Pipe".to_string(),
            Block::PipeSplitXY { .. } => "Pipe split".to_string(),
            Block::PipeMergeXY => "Pipe crossing".to_string(),
            Block::FunnelXY => "Funnel".to_string(),
            Block::WindSource => "Wind source".to_string(),
            Block::BlipSpawn {
                num_spawns: None, ..
            } => "Blip source".to_string(),
            Block::BlipSpawn {
                num_spawns: Some(_),
                ..
            } => "Blip spawn".to_string(),
            Block::BlipDuplicator { kind: Some(_), .. } => "Picky blip copier".to_string(),
            Block::BlipDuplicator { kind: None, .. } => "Blip copier".to_string(),
            Block::BlipWindSource { .. } => "Blipped wind spawn".to_string(),
            Block::Solid => "Solid".to_string(),
            Block::Input { .. } => "Input".to_string(),
            Block::Output { .. } => "Output".to_string(),
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            Block::Pipe(_, _) => "Conducts both wind and blips.",
            Block::PipeSplitXY { .. } => "Useless.",
            Block::PipeMergeXY => "Four-way pipe. But why?",
            Block::FunnelXY => "Not so useful.",
            Block::WindSource => "Produces a stream of wind in all directions.",
            Block::BlipSpawn {
                num_spawns: None, ..
            } => "Produces a stream of blips.",
            Block::BlipSpawn {
                num_spawns: Some(1),
                ..
            } => "Spawns one blip.",
            Block::BlipSpawn {
                num_spawns: Some(_),
                ..
            } => "Spawns a limited number of blips.",
            Block::BlipDuplicator { kind: None, .. } => {
                "Produces two copies of whatever blip activates it."
            }
            Block::BlipDuplicator { kind: Some(_), .. } => {
                "Produces two copies of a specific kind of blip that may activate it."
            }
            Block::BlipWindSource { .. } => "Spawns one thrust of wind when activated by a blip.",
            Block::Solid => "Eats blips.",
            Block::Input { .. } => "Input of the machine.",
            Block::Output { .. } => "Output of the machine.",
        }
    }

    pub fn kind(&self) -> Option<BlipKind> {
        match self {
            Block::BlipSpawn { kind, .. } => Some(*kind),
            Block::BlipDuplicator { kind, .. } => *kind,
            _ => None,
        }
    }

    pub fn with_kind(&self, new_kind: BlipKind) -> Block {
        let mut block = self.clone();

        match block {
            Block::BlipSpawn { ref mut kind, .. } => *kind = new_kind,
            Block::BlipDuplicator { ref mut kind, .. } => *kind = Some(new_kind),
            _ => (),
        }

        block
    }

    pub fn has_wind_hole(&self, dir: Dir3) -> bool {
        match self {
            Block::Pipe(dir_a, dir_b) => dir == *dir_a || dir == *dir_b,
            Block::PipeSplitXY { .. } => {
                dir == Dir3::Y_NEG || dir == Dir3::Y_POS || dir == Dir3::X_POS
            }
            Block::PipeMergeXY => dir != Dir3::Z_NEG && dir != Dir3::Z_POS,
            Block::FunnelXY => {
                // Has restricted cases for in/out below
                dir == Dir3::Y_NEG || dir == Dir3::Y_POS
            }
            Block::WindSource => true,
            Block::BlipSpawn { .. } => true,
            Block::BlipDuplicator { .. } => true,
            Block::Solid => true,
            Block::BlipWindSource { .. } => true,
            Block::Input { .. } => dir == Dir3::X_POS,
            Block::Output { .. } => dir != Dir3::Z_NEG,
        }
    }

    pub fn has_wind_hole_in(&self, dir: Dir3) -> bool {
        match self {
            Block::FunnelXY => dir == Dir3::Y_NEG,
            Block::WindSource => false,
            _ => self.has_wind_hole(dir),
        }
    }

    pub fn has_wind_hole_out(&self, dir: Dir3) -> bool {
        match self {
            Block::FunnelXY => dir == Dir3::Y_POS,
            Block::BlipDuplicator { .. } => false,
            Block::BlipWindSource { .. } => {
                // No wind out in the direction of our activating button
                dir != Dir3::Y_NEG
            }
            Block::Output { .. } => false,
            _ => self.has_wind_hole(dir),
        }
    }

    pub fn has_move_hole(&self, dir: Dir3) -> bool {
        match self {
            Block::PipeSplitXY { open_move_hole_y } => {
                dir == Dir3(Axis3::Y, *open_move_hole_y) || dir == Dir3::X_POS
            }
            Block::BlipDuplicator { .. } => dir != Dir3::X_NEG || dir != Dir3::X_POS,
            Block::BlipWindSource { .. } => dir == Dir3::Y_NEG,
            _ => self.has_wind_hole(dir),
        }
    }
}

#[derive(PartialEq, Eq, Clone, Debug, Serialize, Deserialize)]
pub struct PlacedBlock {
    pub rotation_xy: usize,
    pub block: Block,
}

impl PlacedBlock {
    pub fn rotate_cw_xy(&mut self) {
        self.rotation_xy += 1;
        if self.rotation_xy == 4 {
            self.rotation_xy = 0;
        }
    }

    pub fn rotate_ccw_xy(&mut self) {
        if self.rotation_xy == 0 {
            self.rotation_xy = 3;
        } else {
            self.rotation_xy -= 1;
        }
    }

    pub fn rotated_dir_xy(&self, mut dir: Dir3) -> Dir3 {
        for _ in 0..self.rotation_xy {
            dir = dir.rotated_cw_xy();
        }

        dir
    }

    pub fn rotated_dir_ccw_xy(&self, mut dir: Dir3) -> Dir3 {
        for _ in 0..self.rotation_xy {
            dir = dir.rotated_ccw_xy();
        }

        dir
    }

    pub fn angle_xy_radians(&self) -> f32 {
        -std::f32::consts::PI / 2.0 * self.rotation_xy as f32
    }

    pub fn has_wind_hole(&self, dir: Dir3) -> bool {
        self.block.has_wind_hole(self.rotated_dir_ccw_xy(dir))
    }

    pub fn has_move_hole(&self, dir: Dir3) -> bool {
        self.block.has_move_hole(self.rotated_dir_ccw_xy(dir))
    }

    pub fn has_wind_hole_in(&self, dir: Dir3) -> bool {
        self.block.has_wind_hole_in(self.rotated_dir_ccw_xy(dir))
    }

    pub fn has_wind_hole_out(&self, dir: Dir3) -> bool {
        self.block.has_wind_hole_out(self.rotated_dir_ccw_xy(dir))
    }

    pub fn wind_holes(&self) -> Vec<Dir3> {
        // TODO: This could return an iterator to simplify optimizations
        // (or we could use generators, but they don't seem to be stable yet).

        Dir3::ALL
            .iter()
            .filter(|dir| self.has_wind_hole(**dir))
            .copied()
            .collect()
    }

    pub fn wind_holes_in(&self) -> Vec<Dir3> {
        Dir3::ALL
            .iter()
            .filter(|dir| self.has_wind_hole_in(**dir))
            .copied()
            .collect()
    }

    pub fn wind_holes_out(&self) -> Vec<Dir3> {
        Dir3::ALL
            .iter()
            .filter(|dir| self.has_wind_hole_out(**dir))
            .copied()
            .collect()
    }
}

pub type BlockIndex = usize;

#[derive(PartialEq, Eq, Clone, Debug)]
pub struct Blocks {
    // TODO: Make private -- this should not leak for when we extend to chunks
    pub indices: Grid3<Option<BlockIndex>>,
    pub data: VecOption<(Point3, PlacedBlock)>,
}

#[derive(PartialEq, Eq, Clone, Debug)]
pub struct Machine {
    pub blocks: Blocks,
    pub level: Option<Level>,
}

impl Machine {
    pub fn new_from_block_data(
        size: &Vector3,
        slice: &[(Point3, PlacedBlock)],
        level: &Option<Level>,
    ) -> Self {
        let mut indices = Grid3::new(*size);
        let mut data = VecOption::new();

        for (pos, placed_block) in slice {
            indices[*pos] = Some(data.add((*pos, placed_block.clone())));
        }

        let blocks = Blocks { indices, data };

        Machine {
            blocks,
            level: level.clone(),
        }
    }

    pub fn new_sandbox(size: Vector3) -> Self {
        Self {
            blocks: Blocks {
                indices: Grid3::new(size),
                data: VecOption::new(),
            },
            level: None,
        }
    }

    pub fn new_from_level(level: Level) -> Self {
        let mut machine = Self {
            blocks: Blocks {
                indices: Grid3::new(level.size),
                data: VecOption::new(),
            },
            level: Some(level.clone()),
        };

        let input_y_start = level.size.y / 2 - level.spec.input_dim() as isize / 2;

        for index in 0..level.spec.input_dim() {
            machine.set_block_at_pos(
                &Point3::new(0, input_y_start + index as isize, 0),
                Some(PlacedBlock {
                    rotation_xy: 0,
                    block: Block::Input {
                        index,
                        inputs: Vec::new(),
                        activated: None,
                    },
                }),
            );
        }

        let output_y_start = level.size.y / 2 - level.spec.output_dim() as isize / 2;

        for index in 0..level.spec.output_dim() {
            machine.set_block_at_pos(
                &Point3::new(level.size.x - 1, output_y_start + index as isize, 0),
                Some(PlacedBlock {
                    rotation_xy: 0,
                    block: Block::Output {
                        index,
                        expected_next_kind: None,
                    },
                }),
            );
        }

        machine
    }

    pub fn size(&self) -> Vector3 {
        self.blocks.indices.size()
    }

    pub fn is_valid_pos(&self, p: &Point3) -> bool {
        self.blocks.indices.is_valid_pos(p)
    }

    pub fn is_valid_layer(&self, layer: isize) -> bool {
        layer >= 0 && layer < self.size().z
    }

    pub fn get_block_at_pos(&self, p: &Point3) -> Option<(BlockIndex, &PlacedBlock)> {
        self.blocks
            .indices
            .get(p)
            .and_then(|id| *id)
            .map(|id| (id, &self.blocks.data[id].1))
    }

    pub fn get_block_at_pos_mut(&mut self, p: &Point3) -> Option<(BlockIndex, &mut PlacedBlock)> {
        self.blocks
            .indices
            .get(p)
            .and_then(|id| *id)
            .map(move |id| (id, &mut self.blocks.data[id].1))
    }

    pub fn block_at_index(&self, index: BlockIndex) -> &(Point3, PlacedBlock) {
        &self.blocks.data[index]
    }

    pub fn block_pos_at_index(&self, index: BlockIndex) -> Point3 {
        self.blocks.data[index].0
    }

    pub fn set_block_at_pos(&mut self, p: &Point3, block: Option<PlacedBlock>) {
        self.remove_at_pos(p);

        if let Some(block) = block {
            let id = self.blocks.data.add((*p, block));
            self.blocks.indices[*p] = Some(id);
        }
    }

    pub fn remove_at_pos(&mut self, p: &Point3) -> Option<(BlockIndex, PlacedBlock)> {
        if let Some(Some(id)) = self.blocks.indices.get(p).cloned() {
            self.blocks.indices[*p] = None;
            self.blocks.data.remove(id).map(|(data_pos, block)| {
                assert!(data_pos == *p);
                (id, block)
            })
        } else {
            None
        }
    }

    pub fn iter_blocks(&self) -> impl Iterator<Item = (BlockIndex, &(Point3, PlacedBlock))> {
        self.blocks.data.iter()
    }

    pub fn iter_blocks_mut(
        &mut self,
    ) -> impl Iterator<Item = (BlockIndex, &mut (Point3, PlacedBlock))> {
        self.blocks.data.iter_mut()
    }

    pub fn gc(&mut self) {
        self.blocks.data.gc();

        for (index, (grid_pos, _)) in self.blocks.data.iter() {
            self.blocks.indices[*grid_pos] = Some(index);
        }
    }

    pub fn is_contiguous(&self) -> bool {
        self.blocks.data.num_free() == 0
    }

    pub fn num_blocks(&self) -> usize {
        self.blocks.data.len()
    }

    pub fn iter_neighbors<'a>(
        &'a self,
        pos: &'a Point3,
    ) -> impl Iterator<Item = (Dir3, BlockIndex)> + 'a {
        Dir3::ALL.iter().filter_map(move |dir| {
            self.blocks
                .indices
                .get(&(*pos + dir.to_vector()))
                .and_then(|index| index.as_ref())
                .map(|index| (*dir, *index))
        })
    }
}

/// Stores only the data necessary for restoring a machine.
#[derive(PartialEq, Eq, Clone, Debug, Serialize, Deserialize)]
pub struct SavedMachine {
    pub size: Vector3,
    pub block_data: Vec<(Point3, PlacedBlock)>,
    pub level: Option<Level>,
}

impl SavedMachine {
    pub fn from_machine(machine: &Machine) -> Self {
        let block_data = machine
            .blocks
            .data
            .iter()
            .map(|(_index, data)| data.clone())
            .collect();

        Self {
            size: machine.size(),
            block_data,
            level: machine.level.clone(),
        }
    }

    pub fn into_machine(self) -> Machine {
        // TODO: Make use of moving
        Machine::new_from_block_data(&self.size, &self.block_data, &self.level)
    }
}
