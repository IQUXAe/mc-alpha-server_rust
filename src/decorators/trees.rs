use crate::random::JavaRandom;
use super::BlockAccess;

pub struct WorldGenTrees;

impl Default for WorldGenTrees {
    fn default() -> Self {
        Self::new()
    }
}

impl WorldGenTrees {
    pub fn new() -> Self {
        Self
    }

    pub fn generate(&self, accessor: &mut dyn BlockAccess, rand: &mut JavaRandom, x: i32, y: i32, z: i32) -> bool {
        let var6 = rand.next_int_bound(3) + 4;
        let mut var7 = true;

        if y >= 1 && y + var6 < 128 {
            for var8 in y..=(y + 1 + var6) {
                let mut var9 = 1;
                if var8 == y {
                    var9 = 0;
                }
                if var8 >= y + 1 + var6 - 2 {
                    var9 = 2;
                }

                for var10 in (x - var9)..=(x + var9) {
                    for var11 in (z - var9)..=(z + var9) {
                        if (0..128).contains(&var8) {
                            let var12 = accessor.get_block_id(var10, var8, var11);
                            if var12 != 0 && var12 != 18 { // air, leaves
                                var7 = false;
                            }
                        } else {
                            var7 = false;
                        }
                    }
                }
            }

            if !var7 {
                return false;
            }

            let var8 = accessor.get_block_id(x, y - 1, z);
            if (var8 == 2 || var8 == 3) && y < 128 - var6 - 1 {
                accessor.set_block_id(x, y - 1, z, 3); // dirt

                // Leaves
                for var16 in (y - 3 + var6)..=y + var6 {
                    let var10 = var16 - (y + var6);
                    let var11 = 1 - var10 / 2;
                    for var12 in (x - var11)..=(x + var11) {
                        let var13 = var12 - x;
                        for var14 in (z - var11)..=(z + var11) {
                            let var15 = var14 - z;
                            if (var13.abs() != var11 || var15.abs() != var11 || (rand.next_int_bound(2) != 0 && var10 != 0))
                                && !accessor.allows_attachment(var12, var16, var14)
                            {
                                accessor.set_block_id(var12, var16, var14, 18); // leaves
                            }
                        }
                    }
                }

                // Trunk
                for var16 in 0..var6 {
                    let var10 = accessor.get_block_id(x, y + var16, z);
                    if var10 == 0 || var10 == 18 {
                        accessor.set_block_id(x, y + var16, z, 17); // log
                    }
                }
                return true;
            }
        }
        false
    }
}

#[derive(Clone, Copy, Debug)]
struct LeafNode {
    x: i32,
    y: i32,
    z: i32,
    branch_y: i32,
}

pub struct WorldGenBigTree {
    tree_rand: JavaRandom,
    base_pos: [i32; 3],
    height_limit: i32,
    height: i32,
    height_attenuation: f64,
    branch_slope: f64,
    scale_width: f64,
    leaf_density: f64,
    height_limit_limit: i32,
    leaf_distance_limit: i32,
    leaf_nodes: Vec<LeafNode>,
}

const OTHER_COORD_PAIRS: [i8; 6] = [2, 0, 0, 1, 2, 1];

impl Default for WorldGenBigTree {
    fn default() -> Self {
        Self::new()
    }
}

impl WorldGenBigTree {
    pub fn new() -> Self {
        Self {
            tree_rand: JavaRandom::new(0),
            base_pos: [0, 0, 0],
            height_limit: 0,
            height: 0,
            height_attenuation: 0.618,
            branch_slope: 0.381,
            scale_width: 1.0,
            leaf_density: 1.0,
            height_limit_limit: 12,
            leaf_distance_limit: 4,
            leaf_nodes: Vec::new(),
        }
    }

    pub fn configure(&mut self, a: f64, b: f64, c: f64) {
        self.height_limit_limit = (a * 12.0) as i32;
        if a > 0.5 {
            self.leaf_distance_limit = 5;
        }
        self.scale_width = b;
        self.leaf_density = c;
    }

    fn layer_size(&self, y: i32) -> f32 {
        if (y as f64) < (self.height_limit as f64) * 0.3 {
            return -1.618_f32;
        }
        let half_h = (self.height_limit as f32) / 2.0_f32;
        let dist = half_h - (y as f32);
        let mut result = if dist == 0.0_f32 {
            half_h
        } else if dist.abs() >= half_h {
            0.0_f32
        } else {
            (((half_h as f64).powi(2) - (dist.abs() as f64).powi(2)).sqrt()) as f32
        };
        result *= 0.5_f32;
        result
    }

    fn leaf_size(&self, y: i32) -> f32 {
        if y >= 0 && y < self.leaf_distance_limit {
            if y != 0 && y != self.leaf_distance_limit - 1 {
                3.0_f32
            } else {
                2.0_f32
            }
        } else {
            -1.0_f32
        }
    }

    fn check_block_line(&self, accessor: &mut dyn BlockAccess, from: &[i32; 3], to: &[i32; 3]) -> i32 {
        let mut diff = [0; 3];
        let mut max_axis = 0;
        for i in 0..3 {
            diff[i] = to[i] - from[i];
            if diff[i].abs() > diff[max_axis].abs() {
                max_axis = i;
            }
        }
        if diff[max_axis] == 0 {
            return -1;
        }

        let ax1 = OTHER_COORD_PAIRS[max_axis] as usize;
        let ax2 = OTHER_COORD_PAIRS[max_axis + 3] as usize;
        let step = if diff[max_axis] > 0 { 1 } else { -1 };
        let r1 = (diff[ax1] as f64) / (diff[max_axis] as f64);
        let r2 = (diff[ax2] as f64) / (diff[max_axis] as f64);
        let mut pos = [0; 3];
        let mut i = 0;
        let end = diff[max_axis] + step;
        while i != end {
            pos[max_axis] = from[max_axis] + i;
            pos[ax1] = (from[ax1] as f64 + (i as f64) * r1) as i32;
            pos[ax2] = (from[ax2] as f64 + (i as f64) * r2) as i32;
            let bid = accessor.get_block_id(pos[0], pos[1], pos[2]);
            if bid != 0 && bid != 18 {
                break;
            }
            i += step;
        }

        if i == end {
            -1
        } else {
            i.abs()
        }
    }

    fn place_block_line(&self, accessor: &mut dyn BlockAccess, from: &[i32; 3], to: &[i32; 3], block_id: u8) {
        let mut diff = [0; 3];
        let mut max_axis = 0;
        for i in 0..3 {
            diff[i] = to[i] - from[i];
            if diff[i].abs() > diff[max_axis].abs() {
                max_axis = i;
            }
        }
        if diff[max_axis] == 0 {
            return;
        }

        let ax1 = OTHER_COORD_PAIRS[max_axis] as usize;
        let ax2 = OTHER_COORD_PAIRS[max_axis + 3] as usize;
        let step = if diff[max_axis] > 0 { 1 } else { -1 };
        let r1 = (diff[ax1] as f64) / (diff[max_axis] as f64);
        let r2 = (diff[ax2] as f64) / (diff[max_axis] as f64);
        let mut pos = [0; 3];
        let mut i = 0;
        let end = diff[max_axis] + step;
        while i != end {
            pos[max_axis] = ((from[max_axis] + i) as f64 + 0.5).floor() as i32;
            pos[ax1] = (from[ax1] as f64 + (i as f64) * r1 + 0.5).floor() as i32;
            pos[ax2] = (from[ax2] as f64 + (i as f64) * r2 + 0.5).floor() as i32;
            accessor.set_block_id(pos[0], pos[1], pos[2], block_id);
            i += step;
        }
    }

    fn gen_tree_layer(
        &self,
        accessor: &mut dyn BlockAccess,
        center: [i32; 3],
        radius: f32,
        axis: usize,
        block_id: u8,
    ) {
        let int_radius = (radius as f64 + 0.618) as i32;
        let ax1 = OTHER_COORD_PAIRS[axis] as usize;
        let ax2 = OTHER_COORD_PAIRS[axis + 3] as usize;
        let mut pos = [0; 3];
        pos[axis] = center[axis];

        for i in -int_radius..=int_radius {
            pos[ax1] = center[ax1] + i;
            for j in -int_radius..=int_radius {
                let dist = (((i.abs() as f64) + 0.5).powi(2) + ((j.abs() as f64) + 0.5).powi(2)).sqrt();
                if dist > radius as f64 {
                    continue;
                }
                pos[ax2] = center[ax2] + j;
                let bid = accessor.get_block_id(pos[0], pos[1], pos[2]);
                if bid != 0 && bid != 18 {
                    continue;
                }
                accessor.set_block_id(pos[0], pos[1], pos[2], block_id);
            }
        }
    }

    fn generate_leaf_node(&self, accessor: &mut dyn BlockAccess, x: i32, y: i32, z: i32) {
        for i in y..(y + self.leaf_distance_limit) {
            let size = self.leaf_size(i - y);
            self.gen_tree_layer(&mut *accessor, [x, i, z], size, 1, 18); // leaves
        }
    }

    fn generate_leaf_node_list(&mut self, accessor: &mut dyn BlockAccess) {
        self.height = (self.height_limit as f64 * self.height_attenuation) as i32;
        if self.height >= self.height_limit {
            self.height = self.height_limit - 1;
        }

        let mut branch_count = (1.382 + (self.leaf_density * (self.height_limit as f64) / 13.0).powi(2)) as i32;
        if branch_count < 1 {
            branch_count = 1;
        }

        let mut temp = Vec::new();
        let mut top_y = self.base_pos[1] + self.height_limit - self.leaf_distance_limit;
        let base_y = self.base_pos[1] + self.height;
        let mut layer_y = top_y - self.base_pos[1];
        
        temp.push(LeafNode {
            x: self.base_pos[0],
            y: top_y,
            z: self.base_pos[2],
            branch_y: base_y,
        });
        top_y -= 1;

        while layer_y >= 0 {
            let layer_rad = self.layer_size(layer_y);
            if layer_rad < 0.0_f32 {
                top_y -= 1;
                layer_y -= 1;
                continue;
            }

            for _ in 0..branch_count {
                let branch_dist = self.scale_width
                    * (layer_rad as f64)
                    * ((self.tree_rand.next_float() as f64) + 0.328);
                // Vanilla literal (`WorldGenBigTree.java:54` uses 3.14159D,
                // not Math.PI): keep bit-for-bit for 1:1 big-tree shapes.
                #[allow(clippy::approx_constant)]
                let angle = (self.tree_rand.next_float() as f64) * 2.0 * 3.14159_f64;
                let bx = (branch_dist * angle.sin() + (self.base_pos[0] as f64) + 0.5) as i32;
                let bz = (branch_dist * angle.cos() + (self.base_pos[2] as f64) + 0.5) as i32;
                let b_from = [bx, top_y, bz];
                let b_to = [bx, top_y + self.leaf_distance_limit, bz];

                if self.check_block_line(&mut *accessor, &b_from, &b_to) == -1 {
                    let mut trunk = [self.base_pos[0], self.base_pos[1], self.base_pos[2]];
                    let h_dist = (((self.base_pos[0] - bx).abs() as f64).powi(2) + ((self.base_pos[2] - bz).abs() as f64).powi(2)).sqrt();
                    let branch_y = h_dist * self.branch_slope;
                    if (b_from[1] as f64) - branch_y > (base_y as f64) {
                        trunk[1] = base_y;
                    } else {
                        trunk[1] = ((b_from[1] as f64) - branch_y) as i32;
                    }
                    if self.check_block_line(&mut *accessor, &trunk, &b_from) == -1 {
                        temp.push(LeafNode {
                            x: bx,
                            y: top_y,
                            z: bz,
                            branch_y: trunk[1],
                        });
                    }
                }
            }
            top_y -= 1;
            layer_y -= 1;
        }

        self.leaf_nodes = temp;
    }

    fn generate_leaves(&self, accessor: &mut dyn BlockAccess) {
        for node in &self.leaf_nodes {
            self.generate_leaf_node(&mut *accessor, node.x, node.y, node.z);
        }
    }

    fn generate_trunk(&self, accessor: &mut dyn BlockAccess) {
        let from = [self.base_pos[0], self.base_pos[1], self.base_pos[2]];
        let to = [self.base_pos[0], self.base_pos[1] + self.height, self.base_pos[2]];
        self.place_block_line(&mut *accessor, &from, &to, 17); // log
    }

    fn generate_leaf_node_bases(&self, accessor: &mut dyn BlockAccess) {
        let mut trunk = [self.base_pos[0], self.base_pos[1], self.base_pos[2]];
        for node in &self.leaf_nodes {
            let node_pos = [node.x, node.y, node.z];
            trunk[1] = node.branch_y;
            let rel_y = trunk[1] - self.base_pos[1];
            if (rel_y as f64) >= (self.height_limit as f64) * 0.2 {
                self.place_block_line(&mut *accessor, &trunk, &node_pos, 17); // log
            }
        }
    }

    fn valid_tree_location(&mut self, accessor: &mut dyn BlockAccess) -> bool {
        let from = [self.base_pos[0], self.base_pos[1], self.base_pos[2]];
        let to = [self.base_pos[0], self.base_pos[1] + self.height_limit - 1, self.base_pos[2]];
        let below = accessor.get_block_id(self.base_pos[0], self.base_pos[1] - 1, self.base_pos[2]);
        if below != 2 && below != 3 {
            return false; // grass/dirt
        }
        let result = self.check_block_line(&mut *accessor, &from, &to);
        if result == -1 {
            return true;
        }
        if result < 6 {
            return false;
        }
        self.height_limit = result;
        true
    }

    pub fn generate(&mut self, accessor: &mut dyn BlockAccess, rand: &mut JavaRandom, x: i32, y: i32, z: i32) -> bool {
        let lng = rand.next_long();
        self.tree_rand.set_seed(lng);
        self.base_pos = [x, y, z];
        if self.height_limit == 0 {
            self.height_limit = 5 + self.tree_rand.next_int_bound(self.height_limit_limit);
        }
        if !self.valid_tree_location(&mut *accessor) {
            return false;
        }
        self.generate_leaf_node_list(&mut *accessor);
        self.generate_leaves(&mut *accessor);
        self.generate_trunk(&mut *accessor);
        self.generate_leaf_node_bases(&mut *accessor);
        true
    }
}
