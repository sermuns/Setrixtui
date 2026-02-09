//! Game state: playfield, piece, sand, line clear, gravity.

use crate::theme::Theme;
use ratatui::style::Color;
use std::collections::{HashSet, VecDeque};
use std::time::Instant;


/// Scale factor: each tetromino block is GRAIN_SCALE x GRAIN_SCALE grains.
pub const GRAIN_SCALE: usize = 6;

/// Spawn zone: top N physical rows.
const SPAWN_ZONE_ROWS: usize = 2 * GRAIN_SCALE;

/// After this many move/rotate resets, piece locks on next land immediately.
const LOCK_DELAY_RESET_LIMIT: u32 = 15;

/// Tetromino kinds (I, O, T, S, Z, J, L).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TetrominoKind {
    I,
    O,
    T,
    S,
    Z,
    J,
    L,
}

impl TetrominoKind {
    pub const ALL: [Self; 7] = [Self::I, Self::O, Self::T, Self::S, Self::Z, Self::J, Self::L];

    /// 4 cells relative to origin (0,0); each (dx, dy).
    pub fn cells(&self) -> &[(i8, i8); 4] {
        match self {
            Self::I => &[(0, 0), (1, 0), (2, 0), (3, 0)],
            Self::O => &[(0, 0), (1, 0), (0, 1), (1, 1)],
            Self::T => &[(0, 0), (1, 0), (2, 0), (1, 1)],
            Self::S => &[(1, 0), (2, 0), (0, 1), (1, 1)],
            Self::Z => &[(0, 0), (1, 0), (1, 1), (2, 1)],
            Self::J => &[(0, 0), (0, 1), (1, 1), (2, 1)],
            Self::L => &[(2, 0), (0, 1), (1, 1), (2, 1)],
        }
    }

    /// Colour index 0..6 for theme.sand_color().
    /// If high_color is false, maps to 0..3 (Green, Yellow, Red, Blue).
    pub fn color_index(&self, high_color: bool) -> u8 {
        if high_color {
            match self {
                Self::S => 0, // Green
                Self::O => 1, // Yellow
                Self::Z => 2, // Red
                Self::J => 3, // Blue
                Self::T => 4, // Magenta
                Self::I => 5, // Cyan
                Self::L => 2, // Orange -> Red
            }
        } else {
            match self {
                Self::S => 0, // Green
                Self::O => 1, // Yellow
                Self::Z => 2, // Red
                Self::J => 3, // Blue
                Self::T => 2, // Red
                Self::I => 3, // Blue
                Self::L => 1, // Yellow
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrozenGrain {
    pub x: usize,
    pub y: usize,
    pub color_index: u8,
    pub is_shadow: bool,
}

/// Current piece with position and rotation (0..4).
#[derive(Debug, Clone)]
pub struct Piece {
    pub kind: TetrominoKind,
    pub gx: i32,
    pub gy: i32,
    pub rotation: u8, // 0..4
}

impl Piece {
    /// Returns the top-left grain coordinate for each of the 4 tetromino cells.
    pub fn cell_grain_origins(&self) -> [(i32, i32); 4] {
        if self.kind == TetrominoKind::O {
            let s = GRAIN_SCALE as i32;
            return [
                (self.gx, self.gy),
                (self.gx + s, self.gy),
                (self.gx, self.gy + s),
                (self.gx + s, self.gy + s),
            ];
        }

        let cells = self.kind.cells();
        let r = self.rotation % 4;
        let (cx, cy) = match self.kind {
            TetrominoKind::I => (1, 0),
            _ => (1, 1),
        };
        let mut out = [(0i32, 0i32); 4];
        for (i, (dx, dy)) in cells.iter().enumerate() {
            let (rdx, rdy) = rotate_cell(*dx, *dy, r, cx, cy);
            out[i] = (
                self.gx + (rdx as i32 * GRAIN_SCALE as i32),
                self.gy + (rdy as i32 * GRAIN_SCALE as i32),
            );
        }
        out
    }
}

fn rotate_cell(dx: i8, dy: i8, r: u8, cx: i8, cy: i8) -> (i16, i16) {
    let dx = dx - cx;
    let dy = dy - cy;
    let (dx, dy) = match r {
        0 => (dx, dy),
        1 => (-dy, dx),
        2 => (-dx, -dy),
        3 => (dy, -dx),
        _ => (dx, dy),
    };
    (i16::from(dx + cx), i16::from(dy + cy))
}

/// Single cell: either empty or sand of a given colour index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cell {
    Empty,
    Sand(u8, bool), // colour index 0..6, is_shadow
}

/// Playfield: grid of cells. y=0 is top; rows are stored [0..height].
#[derive(Debug, Clone)]
pub struct Playfield {
    pub width: usize,
    pub height: usize,
    /// rows[y][x] = cell. rows[0] is top.
    rows: VecDeque<Vec<Cell>>,
    pub tick_count: u32,
}

impl Playfield {
    pub fn new(width: u16, height: u16) -> Self {
        let (w, h) = (width as usize, height as usize);
        let (gw, gh) = (w * GRAIN_SCALE, h * GRAIN_SCALE);
        let rows = (0..gh).map(|_| vec![Cell::Empty; gw]).collect();
        Self {
            width: w,
            height: h,
            rows,
            tick_count: 0,
        }
    }

    /// Get actual grain dimensions.
    #[inline]
    pub fn grain_dims(&self) -> (usize, usize) {
        (self.width * GRAIN_SCALE, self.height * GRAIN_SCALE)
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize) -> Option<Cell> {
        let (gw, gh) = self.grain_dims();
        if x >= gw || y >= gh { return None; }
        self.rows.get(y).and_then(|row| row.get(x)).copied()
    }

    #[inline]
    pub fn set(&mut self, x: usize, y: usize, cell: Cell) {
        let (gw, gh) = self.grain_dims();
        if x < gw && y < gh {
            if let Some(row) = self.rows.get_mut(y) {
                row[x] = cell;
            }
        }
    }

    /// True if piece can be placed at its current position on the piece-grid.
    /// Checked against the physical grain grid (scaled).
    pub fn can_place(&self, piece: &Piece) -> bool {
        let origins = piece.cell_grain_origins();
        let (gw, gh) = self.grain_dims();
        
        for (gx_origin, gy_origin) in origins {
            for dy in 0..GRAIN_SCALE as i32 {
                for dx in 0..GRAIN_SCALE as i32 {
                    let gx = gx_origin + dx;
                    let gy = gy_origin + dy;
                    
                    // Boundary check
                    if gx < 0 || gx >= gw as i32 || gy >= gh as i32 {
                        return false;
                    }
                    if gy < 0 { continue; }
                    
                    // Collision check
                    if let Some(Cell::Sand(..)) = self.get(gx as usize, gy as usize) {
                        return false;
                    }
                }
            }
        }
        true
    }


    /// Edge-to-edge clear: one colour connects left (x=0) to right (x=width-1); path can be slanted (8-neighbour).
    /// Returns (number of such clears, list of (x,y) cells to clear).
    pub fn find_spanning_components(&self) -> (u32, Vec<(usize, usize)>) {
        const NEIGHBOURS_8: [(i16, i16); 8] = [
            (-1, -1), (-1, 0), (-1, 1),
            (0, -1),           (0, 1),
            (1, -1),  (1, 0),  (1, 1),
        ];
        let (gw, gh) = self.grain_dims();
        let mut num_clears = 0u32;
        let mut all_to_clear = Vec::new();

        for color in 0..6u8 {
            let mut visited = HashSet::new();
            for start_y in 0..gh {
                if let Some(Cell::Sand(c, _)) = self.get(0, start_y) {
                    if c == color && !visited.contains(&(0, start_y)) {
                        let mut component = Vec::new();
                        let mut stack = vec![(0, start_y)];
                        visited.insert((0, start_y));
                        let mut touches_right = false;

                        while let Some((x, y)) = stack.pop() {
                            component.push((x, y));
                            if x == gw - 1 {
                                touches_right = true;
                            }

                            for (dx, dy) in NEIGHBOURS_8 {
                                let nx = x as i16 + dx;
                                let ny = y as i16 + dy;
                                if nx >= 0 && nx < gw as i16 && ny >= 0 && ny < gh as i16 {
                                    let (nx, ny) = (nx as usize, ny as usize);
                                    if let Some(Cell::Sand(c2, _)) = self.get(nx, ny) {
                                        if c2 == color && !visited.contains(&(nx, ny)) {
                                            visited.insert((nx, ny));
                                            stack.push((nx, ny));
                                        }
                                    }
                                }
                            }
                        }

                        if touches_right {
                            num_clears += 1;
                            all_to_clear.extend(component);
                        }
                    }
                }
            }
        }
        (num_clears, all_to_clear)
    }

    /// Unified physics step: gravity + cascading.
    /// Grains fall down, or down-left/down-right if blocked.
    pub fn tick_physics(&mut self, left_first: bool) -> bool {
        self.tick_count = self.tick_count.wrapping_add(1);
        let mut moved = false;
        let (gw, gh) = self.grain_dims();
        // Scan Entropy: Randomize x_order every frame to eliminate clumping bias.
        let mut x_order: Vec<usize> = (0..gw).collect();
        // Uses tick_count for dynamic shuffle
        let seed = self.tick_count.wrapping_mul(31).wrapping_add(gw as u32);
        // Simple swap-based shuffle
        for i in 0..gw/4 {
            let j = (seed as usize + i) % gw;
            let k = (seed as usize * 17 + i) % gw;
            x_order.swap(j, k);
        }
        
        let limit_y = gh.saturating_sub(1);
        for y in (0..limit_y).rev() {
            for &x in &x_order {
                if let Some(Cell::Sand(c, is_shadow)) = self.get(x, y) {
                    // --- STOCHASTIC GRAVITY (Grain Separation) ---
                    // Using tick_count + coordinates ensures every frame is different.
                    let entropy_seed = (x as u32).wrapping_mul(7).wrapping_add(y as u32).wrapping_mul(13).wrapping_add(self.tick_count.wrapping_mul(17));
                    
                    // --- BALANCED GRAVITY REACTIVITY ---
                    // Lower lag (35%) ensures sand feels reactive and falls naturally,
                    // avoiding the "molasses" effect while keeping grains separate.
                    if (entropy_seed % 100) < 35 {
                        continue;
                    }

                    // --- HORIZONTAL DIFFUSION (Dither) ---
                    // 10% chance to drift sideways even if down is clear.
                    // This breaks up mechanical 45-degree staircase patterns.
                    let drift_roll = (entropy_seed / 100) % 100;
                    if drift_roll < 10 {
                        let drift_left = (entropy_seed / 1000) % 2 == 0;
                        if drift_left && x > 0 && self.get(x - 1, y + 1) == Some(Cell::Empty) {
                            self.set(x, y, Cell::Empty);
                            self.set(x - 1, y + 1, Cell::Sand(c, is_shadow));
                            moved = true;
                            continue;
                        } else if !drift_left && x + 1 < gw && self.get(x + 1, y + 1) == Some(Cell::Empty) {
                            self.set(x, y, Cell::Empty);
                            self.set(x + 1, y + 1, Cell::Sand(c, is_shadow));
                            moved = true;
                            continue;
                        }
                    }

                    // 1. Try straight down
                    if self.get(x, y + 1) == Some(Cell::Empty) {
                        self.set(x, y, Cell::Empty);
                        self.set(x, y + 1, Cell::Sand(c, is_shadow));
                        moved = true;
                    } 
                    // 2. Cascading: try down-left or down-right
                    else {
                        let try_left = x > 0 && self.get(x - 1, y + 1) == Some(Cell::Empty);
                        let try_right = x + 1 < gw && self.get(x + 1, y + 1) == Some(Cell::Empty);
                        
                        let go_left = if try_left && try_right {
                            left_first
                        } else {
                            try_left
                        };

                        if go_left {
                            self.set(x, y, Cell::Empty);
                            self.set(x - 1, y + 1, Cell::Sand(c, is_shadow));
                            moved = true;
                        } else if try_right {
                            self.set(x, y, Cell::Empty);
                            self.set(x + 1, y + 1, Cell::Sand(c, is_shadow));
                            moved = true;
                        }
                    }
                }
            }
        }
        moved
    }

    /// Game over if any sand in spawn zone (top SPAWN_ZONE_ROWS).
    pub fn game_over(&self) -> bool {
        let (gw, _gh) = self.grain_dims();
        for y in 0..SPAWN_ZONE_ROWS {
            for x in 0..gw {
                if matches!(self.get(x, y), Some(Cell::Sand(..))) {
                    return true;
                }
            }
        }
        false
    }

}

/// Bag of 7 tetrominoes (random order, then refill).
#[derive(Debug, Clone)]
pub struct Bag {
    queue: Vec<TetrominoKind>,
    rng: u32,
}

impl Bag {
    pub fn new() -> Self {
        let mut b = Self {
            queue: Vec::with_capacity(14),
            rng: 0x1234_5678,
        };
        b.refill();
        b
    }

    fn refill(&mut self) {
        let mut all = TetrominoKind::ALL.to_vec();
        // Fisher–Yates shuffle
        for i in (1..all.len()).rev() {
            let j = (self.next_rand() as usize) % (i + 1);
            all.swap(i, j);
        }
        self.queue.extend(all);
    }

    fn next_rand(&mut self) -> u32 {
        self.rng = self.rng.wrapping_mul(1103515245).wrapping_add(12345);
        self.rng >> 16
    }

    pub fn next(&mut self) -> TetrominoKind {
        if self.queue.len() < 2 {
            self.refill();
        }
        self.queue.remove(0)
    }

}

impl Default for Bag {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct ScorePopup {
    pub x: usize,
    pub y: usize,
    pub amount: u32,
    pub multiplier: u32,
    pub age_ms: u32,
    pub color: Color,
}

/// Game state: playfield, current piece, next piece, score, level, etc.
#[derive(Debug)]
pub struct GameState {
    pub theme: Theme,
    pub playfield: Playfield,
    pub piece: Option<Piece>,
    pub next_pieces: Vec<TetrominoKind>,
    pub bag: Bag,
    pub score: u32,
    pub level: u32,
    pub lines_cleared: u32,
    pub game_over: bool,
    /// Cells to clear (animation); when empty and not in_progress, we clear + gravity.
    pub line_clear_cells: Vec<(usize, usize)>,
    pub line_clear_in_progress: bool,
    /// When piece first landed (can't move down); lock after lock_delay_ms if not reset.
    lock_delay_started: Option<Instant>,
    /// Number of move/rotate resets since last land; cap at LOCK_DELAY_RESET_LIMIT.
    lock_delay_resets: u32,
    /// Spawn delay: piece not controllable / no gravity until this instant (optional).
    spawn_ready_at: Option<Instant>,
    /// Spawn delay in ms (0 = disabled).
    spawn_delay_ms: u64,
    /// High-color mode: if true, uses 6 colors; otherwise 4.
    pub high_color: bool,
    /// Settle direction bias toggle.
    settle_left_first: bool,
    pub difficulty: crate::Difficulty,
    pub popups: Vec<ScorePopup>,
    pub frozen_grains: Vec<FrozenGrain>,
    pub clears: u32,
    pub crumble_delay_ticks: u32,
    pub combo_multiplier: u32,
    pub combo_timer_ticks: u32,
}

impl GameState {
    pub fn new(theme: Theme, width: u16, height: u16, config: &crate::GameConfig) -> Self {
        let mut bag = Bag::new();
        let p1 = bag.next();
        let p2 = bag.next();
        let p3 = bag.next();
        let p4 = bag.next();
        let piece = Some(Self::spawn_piece(width, height, p1));
        let next_pieces = vec![p2, p3, p4];
        
        let now = Instant::now();
        let spawn_ready_at = (config.spawn_delay_ms > 0)
            .then(|| now + std::time::Duration::from_millis(config.spawn_delay_ms));
        Self {
            theme,
            playfield: Playfield::new(width, height),
            piece,
            next_pieces,
            bag,
            score: 0,
            level: config.initial_level,
            lines_cleared: 0,
            game_over: false,
            line_clear_cells: Vec::new(),
            line_clear_in_progress: false,
            lock_delay_started: None,
            lock_delay_resets: 0,
            spawn_ready_at,
            spawn_delay_ms: config.spawn_delay_ms,
            high_color: config.high_color,
            settle_left_first: true,
            difficulty: config.difficulty,
            popups: Vec::new(),
            frozen_grains: Vec::new(),
            clears: 0,
            crumble_delay_ticks: 0,
            combo_multiplier: 1,
            combo_timer_ticks: 0,
        }
    }

    /// True if the current piece is still in spawn delay (no gravity / no input).
    pub fn is_spawn_delay(&self, now: Instant) -> bool {
        self.spawn_ready_at
            .map(|t| now < t)
            .unwrap_or(false)
    }

    pub fn spawn_piece(width: u16, _height: u16, kind: TetrominoKind) -> Piece {
        let w = width as i32;
        let s = GRAIN_SCALE as i32;
        Piece {
            kind,
            gx: (w / 2 - 1).max(0) * s,
            gy: 0,
            rotation: 0,
        }
    }

    /// Move piece down one step if possible.
    pub fn tick_gravity(&mut self, now: Instant) {
        if self.game_over || self.line_clear_in_progress || self.is_spawn_delay(now) {
            return;
        }
        if let Some(ref mut piece) = self.piece {
            piece.gy += 1;
            if !self.playfield.can_place(piece) {
                piece.gy -= 1;
                // Instant crumble! The moment we hit something, it locks.
                self.lock_piece();
            } else {
                // If we moved down successfully, we are NOT landed.
                self.lock_delay_started = None;
                self.lock_delay_resets = 0;
            }
        }
    }

    /// Check if piece should lock due to time spent on ground.
    /// Call this every frame for snappy snapping.
    pub fn check_lock(&mut self, _now: Instant) {
        if self.game_over || self.line_clear_in_progress {
            return;
        }
        if let Some(ref piece) = self.piece {
            let mut test_p = piece.clone();
            test_p.gy += 1;
            
            if !self.playfield.can_place(&test_p) {
                // Piece is on the ground - lock instantly in Sandtrix
                self.lock_piece();
            } else {
                // Piece is in the air
                self.lock_delay_started = None;
                self.lock_delay_resets = 0;
            }
        }
    }

    /// Call when player moves or rotates; resets lock delay and increments reset count.
    pub fn on_move_or_rotate(&mut self, now: Instant) {
        if self.lock_delay_started.is_some() {
            self.lock_delay_started = Some(now);
            self.lock_delay_resets = self.lock_delay_resets.saturating_add(1).min(LOCK_DELAY_RESET_LIMIT);
        }
    }

    pub fn move_left(&mut self, now: Instant) {
        if self.game_over || self.line_clear_in_progress || self.is_spawn_delay(now) {
            return;
        }
        if let Some(ref mut piece) = self.piece {
            piece.gx -= GRAIN_SCALE as i32;
            if !self.playfield.can_place(piece) {
                piece.gx += GRAIN_SCALE as i32;
            }
        }
    }

    pub fn move_right(&mut self, now: Instant) {
        if self.game_over || self.line_clear_in_progress || self.is_spawn_delay(now) {
            return;
        }
        if let Some(ref mut piece) = self.piece {
            piece.gx += GRAIN_SCALE as i32;
            if !self.playfield.can_place(piece) {
                piece.gx -= GRAIN_SCALE as i32;
            }
        }
    }

    /// Wall kick order: try 0, -1, +1, -2, +2 (SRS-style).

    pub fn rotate_cw(&mut self, now: Instant) {
        if self.game_over || self.line_clear_in_progress || self.is_spawn_delay(now) {
            return;
        }
        if let Some(ref mut piece) = self.piece {
            let old_rotation = piece.rotation;
            piece.rotation = (piece.rotation + 1) % 4;
            if !self.playfield.can_place(piece) {
                piece.rotation = old_rotation;
            }
        }
    }

    pub fn rotate_ccw(&mut self, now: Instant) {
        if self.game_over || self.line_clear_in_progress || self.is_spawn_delay(now) {
            return;
        }
        if let Some(ref mut piece) = self.piece {
            let old_rotation = piece.rotation;
            piece.rotation = (piece.rotation + 3) % 4;
            if !self.playfield.can_place(piece) {
                piece.rotation = old_rotation;
            }
        }
    }

    pub fn soft_drop(&mut self, now: Instant) {
        if self.game_over || self.line_clear_in_progress || self.is_spawn_delay(now) {
            return;
        }
        if let Some(ref mut piece) = self.piece {
            piece.gy += 1;
            if !self.playfield.can_place(piece) {
                piece.gy -= 1;
                self.lock_piece();
            } else {
                self.lock_delay_started = None;
                self.lock_delay_resets = 0;
                self.score += 1;
            }
        }
    }

    pub fn hard_drop(&mut self, now: Instant) {
        if self.game_over || self.line_clear_in_progress || self.is_spawn_delay(now) {
            return;
        }
        if let Some(piece) = self.piece.clone() {
            let (_, gh) = self.playfield.grain_dims();
            let mut pgy = piece.gy;
            while pgy < gh as i32 {
                let mut p = piece.clone();
                p.gy = pgy;
                if !self.playfield.can_place(&p) {
                    pgy -= 1;
                    break;
                }
                pgy += 1;
            }
            let dist_grains = (pgy - piece.gy).max(0) as u32;
            self.score += (dist_grains / GRAIN_SCALE as u32) * 2;
            if let Some(ref mut p) = self.piece {
                p.gy = pgy;
            }
            self.lock_piece();
        }
    }

    fn lock_piece(&mut self) {
        let piece = match self.piece.take() {
            Some(p) => p,
            None => return,
        };
        let color_index = piece.kind.color_index(self.high_color);
        
        // --- PIECE FREEZING (Freeze & Crumble) ---
        // Instead of writing to the playfield instantly, we move grains to the frozen buffer.
        // This makes the piece "freeze" in place before dissolving.
        for (gx, gy) in piece.cell_grain_origins() {
            for dy in 0..GRAIN_SCALE as i32 {
                for dx in 0..GRAIN_SCALE as i32 {
                    let px = gx + dx;
                    let py = gy + dy;
                    
                    // Boundary check to prevent grain loss
                    if px >= 0 && py >= 0 {
                        let tx = px as usize;
                        let ty = py as usize;
                        if tx < self.playfield.width * GRAIN_SCALE && ty < self.playfield.height * GRAIN_SCALE {
                            // --- L-SHADOW TAGGING ---
                            // Bottom row OR Right column of each 6x6 block cell is a shadow grain.
                            // This creates persistent edge separation.
                            let is_shadow = (dy == GRAIN_SCALE as i32 - 1) || (dx == GRAIN_SCALE as i32 - 1);
                            
                            self.frozen_grains.push(FrozenGrain {
                                x: tx,
                                y: ty,
                                color_index,
                                is_shadow,
                            });
                        }
                    }
                }
            }
        }

        // --- GRAVITY-FIRST CRUMBLE ---
        // Sort grains by Y ascending so that pop() retrieves the bottom-most grains first.
        // This makes the piece dissolve from the bottom-up naturally.
        self.frozen_grains.sort_by_key(|g| g.y);
        
        self.crumble_delay_ticks = 5; // Freeze for 5 ticks (snappy lock) before crumbling.

        // Trigger line clear check on the playfield
        self.process_clears();
        
        if self.playfield.game_over() {
            self.game_over = true;
            return;
        }
        if !self.line_clear_in_progress {
            self.spawn_next();
        }
    }

    /// Called after line-clear animation: clear cells, apply gravity, spawn next.
    pub fn finish_line_clear(&mut self) {
        if self.line_clear_cells.is_empty() {
            self.line_clear_in_progress = false;
            self.spawn_next();
            return;
        }
        for &(x, y) in &self.line_clear_cells {
            self.playfield.set(x, y, Cell::Empty);
        }
        self.line_clear_cells.clear();
        self.line_clear_in_progress = false;
        // Don't spawn next immediately if there's sand still falling?
        // Actually, we'll let sand fall while the next piece is active.
        self.spawn_next();
    }

    /// Update sand physics (one step). Should be called regularly.
    pub fn tick_sand(&mut self) {
        if self.line_clear_in_progress {
            return;
        }

        // --- PIECE CRUMBLE PROCESSOR ---
        if self.crumble_delay_ticks > 0 {
            self.crumble_delay_ticks = self.crumble_delay_ticks.saturating_sub(1);
        } else {
            // --- TURBO DRAIN (36 grains per tick) ---
            // Faster conversion (one full 6x6 block cell per logic tick).
            for _ in 0..36 {
                if let Some(fg) = self.frozen_grains.pop() {
                    self.playfield.set(fg.x, fg.y, Cell::Sand(fg.color_index, fg.is_shadow));
                }
            }
        }

        // --- COMBO DECAY ---
        if self.combo_timer_ticks > 0 {
            self.combo_timer_ticks = self.combo_timer_ticks.saturating_sub(1);
            if self.combo_timer_ticks == 0 {
                self.combo_multiplier = 1;
            }
        }

        let moved = self.playfield.tick_physics(self.settle_left_first);
        self.settle_left_first = !self.settle_left_first;

        // --- DYNAMIC CLEAR CHECK (During Physics/Crumble) ---
        if (moved || (self.crumble_delay_ticks == 0 && !self.frozen_grains.is_empty())) && !self.line_clear_in_progress {
            self.process_clears();
        }
    }

    /// Check for clears and update score/popups. 
    /// Called after piece lock and during sand flow.
    pub fn process_clears(&mut self) {
        if self.line_clear_in_progress { return; }
        
        let (num, cells) = self.playfield.find_spanning_components();
        if num > 0 {
            // --- COMBO SYSTEM ---
            self.combo_multiplier = (self.combo_multiplier + 1).min(10);
            self.combo_timer_ticks = 90; // 1.5s at 60Hz

            let pixel_score = cells.len() as u32;
            let amount = pixel_score * self.combo_multiplier;
            
            self.score += amount;
            self.lines_cleared += num;
            self.clears += num;
            self.level = 1 + self.lines_cleared / 10;
            
            self.line_clear_cells = cells;
            self.line_clear_in_progress = true;
            
            // Score popup for EVERY clear trigger
            let (px, py) = if !self.line_clear_cells.is_empty() {
                self.line_clear_cells[0]
            } else {
                ((self.playfield.width * GRAIN_SCALE) / 2, (self.playfield.height * GRAIN_SCALE) / 2)
            };
            
            self.popups.push(ScorePopup {
                x: px,
                y: py,
                amount,
                multiplier: self.combo_multiplier,
                age_ms: 0,
                color: Color::Yellow,
            });
        }
    }

    fn spawn_next(&mut self) {
        let width = self.playfield.width as u16;
        let height = self.playfield.height as u16;
        
        // Pull from queue
        let next_kind = self.next_pieces.remove(0);
        // Refill queue
        self.next_pieces.push(self.bag.next());
        
        self.piece = Some(Self::spawn_piece(width, height, next_kind));
        if self.spawn_delay_ms > 0 {
            self.spawn_ready_at = Some(Instant::now() + std::time::Duration::from_millis(self.spawn_delay_ms));
        } else {
            self.spawn_ready_at = None;
        }
        if !self.playfield.can_place(self.piece.as_ref().unwrap()) {
            self.game_over = true;
        }
    }

    pub fn piece_color(&self, kind: TetrominoKind) -> Color {
        self.theme.sand_color(kind.color_index(self.high_color))
    }

    pub fn tick_popups(&mut self, delta_ms: u32) {
        self.popups.retain_mut(|p| {
            let old_steps = p.age_ms / 150;
            p.age_ms += delta_ms;
            let new_steps = p.age_ms / 150;
            if new_steps > old_steps && p.y > 0 {
                p.y = p.y.saturating_sub(1); // Float up smoothly
            }
            p.age_ms < 1500 // Last for 1.5s
        });
    }
}
