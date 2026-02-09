//! Layout and drawing: menu, playfield, pause, game over, next preview, colour strip, score.

use crate::app::{GameOverReason, MenuState, MenuTab, Screen};
use crate::game::{Cell, GameState, TetrominoKind};
use crate::GameMode;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use ratatui::Frame;
use std::collections::HashSet;
use std::time::Instant;
use tachyonfx::{ref_count, CellFilter, Duration as TfxDuration, Effect, EffectRenderer, Interpolation, fx};

/// We use half-blocks (▀) to get 2 grains per terminal cell (vertically).
const CELL_WIDTH: u16 = 1; 
const CELL_HEIGHT: u16 = 1;
/// Playfield grid size in terminal cells (border + grid) for given grid dimensions.
fn playfield_pixel_size(width: u16, height: u16) -> (u16, u16) {
    let scale = crate::game::GRAIN_SCALE as u16;
    let gw = width * scale;
    let gh = height * scale;
    (gw + 2, (gh / 2) + 2)
}

/// Max playfield size (width, height) in grid cells that fit in the given terminal size.
/// Used so --width/--height are clamped and the board + border fit on screen.
pub fn max_playfield_cells_for_terminal(term_cols: u16, term_rows: u16) -> (u16, u16) {
    let scale = crate::game::GRAIN_SCALE as u16;
    let max_pw = term_cols.saturating_sub(2).saturating_sub(SIDEBAR_WIDTH);
    let max_ph = term_rows.saturating_sub(2);
    let max_width = max_pw / scale;
    let max_height = max_ph / (scale / 2);
    (max_width, max_height)
}

/// Minimum playfield size (grid cells). Zooming out can increase size up to MAX.
pub const MIN_PLAYFIELD_WIDTH: u16 = 10;
pub const MIN_PLAYFIELD_HEIGHT: u16 = 24;
/// Maximum playfield size; kept modest so zooming out doesn't make the board insanely wide/tall.
pub const MAX_PLAYFIELD_WIDTH: u16 = 12;
pub const MAX_PLAYFIELD_HEIGHT: u16 = 28;

/// Playfield size that fits the terminal: at most MAX, at least 1. When terminal is small we go below MIN so content always fits (no squeeze).
pub fn playfield_size_for_terminal_clamped(term_cols: u16, term_rows: u16) -> (u16, u16) {
    let (max_w, max_h) = max_playfield_cells_for_terminal(term_cols, term_rows);
    let w = max_w.min(MAX_PLAYFIELD_WIDTH).max(1);
    let h = max_h.min(MAX_PLAYFIELD_HEIGHT).max(1);
    (w, h)
}

/// Color for playfield size indicator: red = minimum, yellow = okay, green = good.
pub fn playfield_size_indicator_color(w: u16, h: u16) -> Color {
    let min_cells = MIN_PLAYFIELD_WIDTH as u32 * MIN_PLAYFIELD_HEIGHT as u32;
    let cells = w as u32 * h as u32;
    if cells <= min_cells {
        Color::Red
    } else if cells <= min_cells * 13 / 10 {
        Color::Yellow
    } else {
        Color::Green
    }
}

const SIDEBAR_WIDTH: u16 = 24;

/// Duration of line-clear fade (TachyonFX) in ms (SPEC §14.1: ~30 ms per grain).
const LINE_CLEAR_FADE_MS: u32 = 400;

/// Playfield inner rect (board only, no border) for given area and state; matches draw_game layout.
fn playfield_board_rect(area: Rect, state: &GameState) -> Rect {
    let (pw, ph) = playfield_pixel_size(state.playfield.width as u16, state.playfield.height as u16);
    let total_w = pw + SIDEBAR_WIDTH;
    let x = area.x + area.width.saturating_sub(total_w) / 2;
    let y = area.y + area.height.saturating_sub(ph) / 2;
    let playfield_outer = Rect {
        x,
        y,
        width: pw.min(area.width),
        height: ph.min(area.height),
    };
    Rect {
        x: playfield_outer.x + 1,
        y: playfield_outer.y + 1,
        width: (state.playfield.width as u16 * CELL_WIDTH).min(playfield_outer.width.saturating_sub(2)),
        height: (state.playfield.height as u16 * CELL_HEIGHT).min(playfield_outer.height.saturating_sub(2)),
    }
}

/// Build set of buffer (x, y) positions that belong to clearing cells.
fn clearing_buffer_positions(board_rect: Rect, line_clear_cells: &[(usize, usize)]) -> HashSet<(u16, u16)> {
    let mut set = HashSet::new();
    for &(gx, gy) in line_clear_cells {
        let x0 = board_rect.x + (gx as u16) * CELL_WIDTH;
        let y0 = board_rect.y + (gy as u16) * CELL_HEIGHT;
        for bx in x0..(x0 + CELL_WIDTH).min(board_rect.x + board_rect.width) {
            for by in y0..(y0 + CELL_HEIGHT).min(board_rect.y + board_rect.height) {
                set.insert((bx, by));
            }
        }
    }
    set
}

fn apply_shading(color: Color, gx: usize, gy: usize, state: &GameState) -> Color {
    let s = crate::game::GRAIN_SCALE;
    let lx = gx % s;
    let ly = gy % s;
    
    // 1. Organic Radial Dome (Pebble look)
    // Distance from top-left light source.
    let fx = lx as f32;
    let fy = ly as f32;
    // Normalize coordinates to 0.0 - 1.0 within the grain.
    let nx = (fx + 0.5) / s as f32;
    let ny = (fy + 0.5) / s as f32;
    let dist = (nx * nx + ny * ny).sqrt() / 1.414;
    
    // Smooth spherical highlight (Softer range for natural look)
    // Range: 1.08 (center-top-left) to 0.94 (bottom-right)
    let bevel_factor = 1.09 - (dist * 0.15); 
    let bevel_factor = bevel_factor.clamp(0.92, 1.12);

    // 2. Natural Edge Detection (Ambient Occlusion)
    let mut edge_darkness = 1.0;
    
    // Check neighbors at 4x4 grain boundaries
    if lx == 0 || lx == s-1 || ly == 0 || ly == s-1 {
        let current_cell = state.playfield.get(gx, gy);
        let (gw, gh) = state.playfield.grain_dims();
        
        let neighbor_check = match (lx, ly) {
            (0, _) if gx > 0 => Some((gx - 1, gy)),
            (x, _) if x == s - 1 && gx + 1 < gw => Some((gx + 1, gy)),
            (_, 0) if gy > 0 => Some((gx, gy - 1)),
            (_, y) if y == s - 1 && gy + 1 < gh => Some((gx, gy + 1)),
            _ => None,
        };

        if let Some((nx, ny)) = neighbor_check {
            if state.playfield.get(nx, ny) != current_cell {
                // Soft shadow perimeter to define grains without being boxy
                edge_darkness = 0.95;
            }
        }
    }

    // 3. Bottom-Right "L" Shadow (Piece Differentiation)
    let mut final_factor = bevel_factor * edge_darkness;
    
    // Check if this grain is a shadow (either from playfield or piece)
    let is_shadow = if let Some(crate::game::Cell::Sand(_, s)) = state.playfield.get(gx, gy) {
        s
    } else if let Some(ref piece) = state.piece {
        // Dynamic check for piece shadow based on 6x6 grid
        let mut piece_shadow = false;
        for (pgx, pgy) in piece.cell_grain_origins() {
            if gx as i32 >= pgx && (gx as i32) < pgx + s as i32 &&
               gy as i32 >= pgy && (gy as i32) < pgy + s as i32 {
                let dx = gx as i32 - pgx;
                let dy = gy as i32 - pgy;
                if dx == s as i32 - 1 || dy == s as i32 - 1 {
                    piece_shadow = true;
                }
                break;
            }
        }
        piece_shadow
    } else {
        false
    };

    if is_shadow {
        final_factor *= 0.70; // Slightly deeper shadow for vibrant colors
    }

    // Simple RGB scaling
    let (r, g, b) = match color {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Red => (255, 0, 0),
        Color::Green => (0, 255, 0),
        Color::Yellow => (255, 255, 0),
        Color::Blue => (0, 0, 255),
        Color::Magenta => (255, 0, 255),
        Color::Cyan => (0, 255, 255),
        Color::Gray => (128, 128, 128),
        Color::DarkGray => (64, 64, 64),
        Color::White => (255, 255, 255),
        _ => (128, 128, 128),
    };

    Color::Rgb(
        (r as f32 * final_factor).min(255.0) as u8,
        (g as f32 * final_factor).min(255.0) as u8,
        (b as f32 * final_factor).min(255.0) as u8,
    )
}

/// Create or update line-clear fade effect and process it (TachyonFX: fade clearing cells to bg over ~30 ms).
fn apply_line_clear_effect(
    frame: &mut Frame,
    state: &GameState,
    area: Rect,
    line_clear_effect: &mut Option<Effect>,
    line_clear_process_time: &mut Option<Instant>,
    now: Instant,
) {
    let board_rect = playfield_board_rect(area, state);
    let delta = line_clear_process_time
        .map(|t| now.saturating_duration_since(t))
        .unwrap_or(std::time::Duration::ZERO);
    let delta_ms = delta.as_millis().min(u32::MAX as u128) as u32;
    let tfx_delta = TfxDuration::from_millis(delta_ms);
    *line_clear_process_time = Some(now);

    if line_clear_effect.is_none() {
        let clearing_set = clearing_buffer_positions(board_rect, &state.line_clear_cells);
        let filter = CellFilter::PositionFn(ref_count(move |pos: Position| {
            clearing_set.contains(&(pos.x, pos.y))
        }));
        let bg = state.theme.bg;
        let effect = fx::fade_to(bg, bg, (LINE_CLEAR_FADE_MS, Interpolation::Linear))
            .with_filter(filter)
            .with_area(board_rect);
        *line_clear_effect = Some(effect);
    }

    if let Some(effect) = line_clear_effect {
        frame.render_effect(effect, board_rect, tfx_delta);
    }
}

/// Next preview: small grid.
const NEXT_PREVIEW_COLS: u16 = 4;
const NEXT_PREVIEW_ROWS: u16 = 2;
const NEXT_MINI_CELL_W: u16 = 2;
const NEXT_MINI_CELL_H: u16 = 1;

/// Draw current screen (menu, game, game over), with optional pause overlay and game-over reason.
/// When `line_clear_in_progress` and !no_animation, applies TachyonFX fade effect and updates
/// `line_clear_effect` / `line_clear_process_time`.
/// When on menu, `menu_playfield_size` is Some((w, h)) for the playfield size that will be used if the user starts (zoom out = bigger).
pub fn draw(
    frame: &mut Frame,
    screen: Screen,
    state: &GameState,
    paused: bool,
    game_over_reason: Option<GameOverReason>,
    mode: GameMode,
    clear_lines: u32,
    time_limit: u32,
    game_start: Instant,
    area: Rect,
    line_clear_effect: &mut Option<Effect>,
    line_clear_process_time: &mut Option<Instant>,
    menu_state: &mut MenuState,
    now: Instant,
    no_animation: bool,
    quit_selected: Option<crate::app::QuitOption>,
    menu_playfield_size: Option<(u16, u16)>,
) {
    match screen {
        Screen::Menu => draw_menu(frame, state, menu_state, area, now, menu_playfield_size),
        Screen::Playing => {
            draw_game(frame, state, area, mode, time_limit, game_start, now);
            if paused {
                draw_pause_overlay(frame, state, area);
            }
            if state.line_clear_in_progress
                && !state.line_clear_cells.is_empty()
                && !no_animation
            {
                apply_line_clear_effect(
                    frame,
                    state,
                    area,
                    line_clear_effect,
                    line_clear_process_time,
                    now,
                );
            }
        }
        Screen::QuitMenu => {
            draw_game(frame, state, area, mode, time_limit, game_start, now);
            if let Some(opt) = quit_selected {
                draw_quit_menu(frame, state, opt);
            }
        }
        Screen::GameOver => draw_game_over(
            frame,
            state,
            game_over_reason,
            mode,
            clear_lines,
            time_limit,
            game_start,
            area,
        ),
    }
}

fn draw_menu(
    frame: &mut Frame,
    state: &GameState,
    menu_state: &MenuState,
    area: Rect,
    now: Instant,
    menu_playfield_size: Option<(u16, u16)>,
) {
    let popup_w = 48u16;
    let popup_h = if menu_playfield_size.is_some() { 22 } else { 20 };
    let popup = Rect {
        x: area.x + area.width.saturating_sub(popup_w) / 2,
        y: area.y + area.height.saturating_sub(popup_h) / 2,
        width: popup_w.min(area.width),
        height: popup_h.min(area.height),
    };

    // Dynamic Neon Title
    let title = Line::from(vec![
        Span::styled(" Setrix ", Style::default().fg(Color::Rgb(255, 120, 120)).bold()),
        Span::styled(" tui ", Style::default().fg(state.theme.main_fg).bold()),
    ]);

    let ratman_style = if menu_state.ratman_unlocked {
        Style::default().fg(Color::Rgb(255, 0, 255)).bold().italic()
    } else {
        Style::default().fg(state.theme.bg)
    };
    
    let ratman_tag = if menu_state.ratman_unlocked {
        Line::from(vec![
            Span::styled(" [ RATMAN ENCRYPTED MODE ENABLED ] ", ratman_style),
        ])
    } else {
        Line::from("")
    };

    let highlight_style = Style::default().fg(Color::Black).bg(state.theme.sand_color(1)).bold();
    let selected_style = Style::default().fg(state.theme.sand_color(1)).bold();
    let normal_style = Style::default().fg(state.theme.main_fg);

    fn tab_style(current: bool, selected: bool, highlight: Style, select: Style, normal: Style) -> Style {
        if current { highlight }
        else if selected { select }
        else { normal }
    }

    let diff_easy = Span::styled(" EASY ", tab_style(
        menu_state.current_tab == MenuTab::Difficulty && menu_state.selected_difficulty == crate::Difficulty::Easy,
        menu_state.selected_difficulty == crate::Difficulty::Easy,
        highlight_style, selected_style, normal_style
    ));
    let diff_med = Span::styled(" MEDIUM ", tab_style(
        menu_state.current_tab == MenuTab::Difficulty && menu_state.selected_difficulty == crate::Difficulty::Medium,
        menu_state.selected_difficulty == crate::Difficulty::Medium,
        highlight_style, selected_style, normal_style
    ));
    let diff_hard = Span::styled(" HARD ", tab_style(
        menu_state.current_tab == MenuTab::Difficulty && menu_state.selected_difficulty == crate::Difficulty::Hard,
        menu_state.selected_difficulty == crate::Difficulty::Hard,
        highlight_style, selected_style, normal_style
    ));

    let mode_endless = Span::styled(" ENDLESS ", tab_style(
        menu_state.current_tab == MenuTab::Mode && menu_state.selected_mode == crate::GameMode::Endless,
        menu_state.selected_mode == crate::GameMode::Endless,
        highlight_style, selected_style, normal_style
    ));
    let mode_timed = Span::styled(" TIMED ", tab_style(
        menu_state.current_tab == MenuTab::Mode && menu_state.selected_mode == crate::GameMode::Timed,
        menu_state.selected_mode == crate::GameMode::Timed,
        highlight_style, selected_style, normal_style
    ));
    let mode_clear = Span::styled(" CLEAR ", tab_style(
        menu_state.current_tab == MenuTab::Mode && menu_state.selected_mode == crate::GameMode::Clear,
        menu_state.selected_mode == crate::GameMode::Clear,
        highlight_style, selected_style, normal_style
    ));

    let start_btn = if menu_state.current_tab == MenuTab::Start {
        Span::styled(" [ START SIMULATION ] ", highlight_style)
    } else {
        Span::styled(" [ START SIMULATION ] ", normal_style)
    };

    let playfield_size_line = menu_playfield_size.map(|(w, h)| {
        let color = playfield_size_indicator_color(w, h);
        Line::from(Span::styled(
            format!(" Playfield {}×{} ", w, h),
            Style::default().fg(color).bold(),
        ))
    });

    let mut lines = vec![
        Line::from(""),
        title,
        ratman_tag,
        Line::from(""),
    ];
    if let Some(line) = playfield_size_line {
        lines.push(line);
        lines.push(Line::from(""));
    }
    lines.extend([
        Line::from(""),
        Line::from(Span::styled(" ─ SYSTEM DIFFICULTY ─ ", Style::default().fg(state.theme.div_line))),
        Line::from(vec![diff_easy, Span::from("  "), diff_med, Span::from("  "), diff_hard]),
        Line::from(""),
        Line::from(Span::styled(" ─ MISSION MODE ─ ", Style::default().fg(state.theme.div_line))),
        Line::from(vec![mode_endless, Span::from("  "), mode_timed, Span::from("  "), mode_clear]),
        Line::from(""),
        Line::from(""),
        Line::from(start_btn),
        Line::from(""),
        Line::from(""),
        Line::from(vec![
            Span::styled(" ↕ ", Style::default().fg(state.theme.sand_color(3))),
            Span::from("NAVIGATE   "),
            Span::styled(" ↔ ", Style::default().fg(state.theme.sand_color(3))),
            Span::from("CHANGE   "),
            Span::styled(" ENTER ", Style::default().fg(state.theme.sand_color(3))),
            Span::from("INITIALIZE"),
        ]),
        Line::from(""),
        Line::from(Span::styled(" ⌁ [Q] ABORT SESSION ", Style::default().fg(Color::Rgb(255, 80, 80)))),
    ]);

    let p = Paragraph::new(lines)
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(state.theme.div_line).bg(state.theme.bg))
        );

    // Startup animation: slide in from bottom
    let elapsed = now.duration_since(menu_state.animation_start).as_millis() as u32;
    let anim_duration = 500u32;
    let t = (elapsed as f32 / anim_duration as f32).min(1.0);
    // Ease out cubic
    let offset_t = 1.0 - (1.0 - t).powi(3);
    
    let anim_y_offset = ((1.0 - offset_t) * 10.0) as u16;
    let mut anim_popup = popup;
    anim_popup.y += anim_y_offset;

    if t < 1.0 {
        // Fade in effect
        let _alpha = (t * 255.0) as u8;
        // Simple manual fade: apply opacity to block border if we could, 
        // but for TUI we just render and use effect if possible.
        // Actually TachyonFX is better here.
    }

    p.render(anim_popup, frame.buffer_mut());

    if !state.game_over && elapsed < anim_duration {
        // Trigger redraw
    }
}

fn draw_pause_overlay(frame: &mut Frame, state: &GameState, area: Rect) {
    let popup_w = 28u16;
    let popup_h = 5u16;
    let popup = Rect {
        x: area.x + area.width.saturating_sub(popup_w) / 2,
        y: area.y + area.height.saturating_sub(popup_h) / 2,
        width: popup_w.min(area.width),
        height: popup_h.min(area.height),
    };
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            " Paused ",
            Style::default().fg(Color::Black).bg(Color::Yellow),
        )),
        Line::from(""),
        Line::from(Span::styled(
            " P — Resume    Q — Quit ",
            Style::default().fg(state.theme.main_fg),
        )),
    ];
    let p = Paragraph::new(lines)
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(state.theme.div_line).bg(state.theme.bg)),
        );
    p.render(popup, frame.buffer_mut());
}

fn draw_game_over(
    frame: &mut Frame,
    state: &GameState,
    reason: Option<GameOverReason>,
    _mode: GameMode,
    clear_lines: u32,
    time_limit: u32,
    game_start: Instant,
    area: Rect,
) {
    let (pw, ph) = playfield_pixel_size(state.playfield.width as u16, state.playfield.height as u16);
    let total_w = pw + SIDEBAR_WIDTH;
    let total_h = ph;
    let x = area.x + area.width.saturating_sub(total_w) / 2;
    let y = area.y + area.height.saturating_sub(total_h) / 2;
    let popup = Rect {
        x,
        y,
        width: total_w.min(area.width),
        height: total_h.min(area.height),
    };
    let title = match reason {
        Some(GameOverReason::ClearedN) => " You win! ",
        Some(GameOverReason::TimeUp) => " Time's up! ",
        _ => " Game Over ",
    };
    let mut lines: Vec<Line> = vec![
        Line::from(""),
        Line::from(Span::styled(
            title,
            Style::default().fg(Color::White).bg(Color::Red),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!(" Score: {} ", state.score),
            Style::default().fg(state.theme.main_fg),
        )),
        Line::from(Span::styled(
            format!(" Lines: {} ", state.lines_cleared),
            Style::default().fg(state.theme.main_fg),
        )),
    ];
    if reason == Some(GameOverReason::TimeUp) {
        let elapsed = game_start.elapsed().as_secs();
        lines.push(Line::from(Span::styled(
            format!(" Time: {} / {} sec ", elapsed, time_limit),
            Style::default().fg(state.theme.main_fg),
        )));
    } else if reason == Some(GameOverReason::ClearedN) {
        lines.push(Line::from(Span::styled(
            format!(" Target: {} lines ", clear_lines),
            Style::default().fg(state.theme.main_fg),
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " R — Restart    Q — Quit ",
        Style::default().fg(state.theme.main_fg),
    )));
    lines.push(Line::from(""));
    let p = Paragraph::new(lines)
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(state.theme.div_line).bg(state.theme.bg))
                .title(Span::styled(" Setrixtui ", state.theme.title)),
        );
    p.render(popup, frame.buffer_mut());
}

/// Draw game: playfield + sidebar; use full area and center the board.
fn draw_game(
    frame: &mut Frame,
    state: &GameState,
    area: Rect,
    mode: GameMode,
    time_limit: u32,
    game_start: Instant,
    now: Instant,
) {
    let (pw, ph) = playfield_pixel_size(state.playfield.width as u16, state.playfield.height as u16);
    let total_w = pw + SIDEBAR_WIDTH;

    // Center horizontally
    let horiz_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(total_w),
            Constraint::Fill(1),
        ])
        .split(area);

    let center_horiz = horiz_chunks[1];

    // Center vertically
    let vert_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(ph),
            Constraint::Fill(1),
        ])
        .split(center_horiz);

    let active_area = vert_chunks[1];

    let (playfield_area, sidebar_area) = {
        let inner = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(pw), Constraint::Length(SIDEBAR_WIDTH)])
            .split(active_area);
        (inner[0], inner[1])
    };

    draw_playfield(frame, state, playfield_area, mode, time_limit, game_start, now);
    draw_sidebar(frame, state, sidebar_area);
}

fn draw_playfield(
    frame: &mut Frame,
    state: &GameState,
    area: Rect,
    mode: GameMode,
    time_limit: u32,
    game_start: Instant,
    now: Instant,
) {
    let timer_str = if mode == GameMode::Timed {
        let elapsed = now.duration_since(game_start).as_secs();
        let remaining = (time_limit as u64).saturating_sub(elapsed);
        format!(" Time: {:02}:{:02} ", remaining / 60, remaining % 60)
    } else {
        "".to_string()
    };
    
    let title = format!(" Setrixtui{} | Clears: {} ", timer_str, state.clears);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(state.theme.div_line).bg(state.theme.bg))
        .title(Span::styled(title, state.theme.title));
    let inner = block.inner(area);
    block.render(area, frame.buffer_mut());

    let (gw, gh) = state.playfield.grain_dims();
    let board_rect = Rect {
        x: inner.x,
        y: inner.y,
        width: (gw as u16).min(inner.width),
        height: ((gh / 2) as u16).min(inner.height),
    };

    let clear_set: std::collections::HashSet<(usize, usize)> =
        state.line_clear_cells.iter().copied().collect();
    let flashing = state.line_clear_in_progress && !state.line_clear_cells.is_empty();

    let buf = frame.buffer_mut();

    // Iterate by terminal rows (y step 2)
    for y in (0..gh).step_by(2) {
        for x in 0..gw {
            let top_grain = state.playfield.get(x, y);
            let bot_grain = state.playfield.get(x, y + 1);
            
            let is_top_clearing = flashing && clear_set.contains(&(x, y));
            let is_bot_clearing = flashing && clear_set.contains(&(x, y + 1));

            // Check if piece is at these grain locations
            let top_piece_color = get_piece_at_grain(state, x, y);
            let bot_piece_color = get_piece_at_grain(state, x, y + 1);

            let top_color = if is_top_clearing { Color::White } else {
                top_piece_color.unwrap_or_else(|| {
                    match top_grain {
                        Some(Cell::Sand(i, _)) => apply_shading(state.theme.sand_color(i), x, y, state),
                        _ => state.theme.bg,
                    }
                })
            };
            let bot_color = if is_bot_clearing { Color::White } else {
                bot_piece_color.unwrap_or_else(|| {
                    match bot_grain {
                        Some(Cell::Sand(i, _)) => apply_shading(state.theme.sand_color(i), x, y + 1, state),
                        _ => state.theme.bg,
                    }
                })
            };

            let rx = board_rect.x + x as u16;
            let ry = board_rect.y + (y / 2) as u16;

            if rx < board_rect.x + board_rect.width && ry < board_rect.y + board_rect.height {
                buf[(rx, ry)]
                    .set_symbol("▀")
                    .set_style(Style::default().fg(top_color).bg(bot_color));
            }
        }
    }

    // Draw Frozen Pieces (Crumbling)
    for fg in &state.frozen_grains {
        let rx = board_rect.x + (fg.x as u16);
        let ry = board_rect.y + (fg.y as u16 / 2);
        if rx < board_rect.x + board_rect.width && ry < board_rect.y + board_rect.height {
            let color = apply_shading(state.theme.sand_color(fg.color_index), fg.x, fg.y, state);
            let style = Style::default().fg(color).bg(color);
            // Frozen grains use a solid block to look "frozen"
            buf[(rx, ry)].set_symbol("█").set_style(style);
        }
    }

    // Draw Floating Score Popups!
    for popup in &state.popups {
        let rx = board_rect.x + (popup.x as u16);
        let ry = board_rect.y + (popup.y as u16 / 2);
        if rx < board_rect.x + board_rect.width && ry < board_rect.y + board_rect.height {
            let label = if popup.multiplier > 1 {
                format!("+{} (x{})", popup.amount, popup.multiplier)
            } else {
                format!("+{}", popup.amount)
            };
            let style = Style::default().fg(popup.color).bg(state.theme.bg).bold();
            frame.buffer_mut().set_string(rx, ry, label, style);
        }
    }
}

fn get_piece_at_grain(state: &GameState, gx: usize, gy: usize) -> Option<Color> {
    if let Some(ref piece) = state.piece {
        for (pgx, pgy) in piece.cell_grain_origins() {
            if gx as i32 >= pgx && (gx as i32) < pgx + crate::game::GRAIN_SCALE as i32 &&
               gy as i32 >= pgy && (gy as i32) < pgy + crate::game::GRAIN_SCALE as i32 {
                let color = state.piece_color(piece.kind);
                // Shading is now aware of piece grains too
                return Some(apply_shading(color, gx, gy, state));
            }
        }
    }
    None
}


fn draw_sidebar(frame: &mut Frame, state: &GameState, area: Rect) {
    let title_style = Style::default().fg(state.theme.title);
    let fg_style = Style::default().fg(state.theme.main_fg);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(state.theme.div_line).bg(state.theme.bg));
    let inner = block.inner(area);
    block.render(area, frame.buffer_mut());

    let sidebar_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Next Title
            Constraint::Length(5), // Previews (side-by-side)
            Constraint::Length(1), // Spacer
            Constraint::Length(1), // Colours Title
            Constraint::Length(1), // Colour Strip
            Constraint::Length(1), // Spacer
            Constraint::Min(4),    // Stats
        ])
        .split(inner);

    let next_label_rect = sidebar_chunks[0];
    let next_preview_rect = sidebar_chunks[1];
    let colours_label_rect = sidebar_chunks[3];
    let colours_strip_rect = sidebar_chunks[4];
    let stats_rect = sidebar_chunks[6];

    let next_label = Paragraph::new(Line::from(Span::styled("Next", title_style)));
    next_label.render(next_label_rect, frame.buffer_mut());

    draw_next_preview(frame, state, next_preview_rect);

    let colours_label = Paragraph::new(Line::from(Span::styled("Colours", title_style)));
    colours_label.render(colours_label_rect, frame.buffer_mut());
    draw_colour_strip(frame, state, colours_strip_rect);

    let score_line = Line::from(vec![
        Span::styled("Score: ", title_style),
        Span::styled(state.score.to_string(), fg_style),
    ]);
    let level_line = Line::from(vec![
        Span::styled("Level: ", title_style),
        Span::styled(state.level.to_string(), fg_style),
    ]);
    let clears_line = Line::from(vec![
        Span::styled("Clears: ", title_style),
        Span::styled(state.clears.to_string(), fg_style),
    ]);
    let mut lines = vec![score_line, level_line, clears_line];
    if state.combo_multiplier > 1 {
        lines.push(Line::from(vec![
            Span::styled("Combo: ", title_style),
            Span::styled(format!("x{}", state.combo_multiplier), Style::default().fg(Color::Yellow).add_modifier(ratatui::style::Modifier::BOLD)),
        ]));
    }
    let p = Paragraph::new(ratatui::text::Text::from(lines));
    p.render(stats_rect, frame.buffer_mut());
}

/// Draw next piece as a small block preview (actual shape).
fn draw_next_preview(frame: &mut Frame, state: &GameState, area: Rect) {
    let num_previews = match state.difficulty {
        crate::Difficulty::Easy => 3,
        crate::Difficulty::Medium => 2,
        crate::Difficulty::Hard => 1,
    };
    
    // Side-by-side: each preview gets a fixed width
    let pw = 7;
    for i in 0..num_previews {
        if i >= state.next_pieces.len() { break; }
        let kind = state.next_pieces[i];
        let sub_area = Rect {
            x: area.x + (i as u16 * pw),
            y: area.y,
            width: pw,
            height: area.height,
        };
        draw_single_piece_preview(frame, state, sub_area, kind);
    }
}

fn draw_single_piece_preview(frame: &mut Frame, state: &GameState, area: Rect, kind: TetrominoKind) {
    let inner = Rect {
        x: area.x,
        y: area.y,
        width: area.width.min(NEXT_PREVIEW_COLS * NEXT_MINI_CELL_W),
        height: area.height.min(NEXT_PREVIEW_ROWS * NEXT_MINI_CELL_H),
    };

    let color = piece_color_static(state, kind);
    let cells = kind.cells();
    let (min_dx, min_dy) = cells
        .iter()
        .fold((i8::MAX, i8::MAX), |(ax, ay): (i8, i8), (dx, dy)| (ax.min(*dx), ay.min(*dy)));
    let (max_dx, max_dy) = cells
        .iter()
        .fold((i8::MIN, i8::MIN), |(ax, ay): (i8, i8), (dx, dy)| (ax.max(*dx), ay.max(*dy)));
    
    let bw = (max_dx - min_dx + 1) as u16;
    let bh = (max_dy - min_dy + 1) as u16;
    let off_x = (inner.width.saturating_sub(bw * NEXT_MINI_CELL_W)) / 2;
    let off_y = (inner.height.saturating_sub(bh * NEXT_MINI_CELL_H)) / 2;

    for (dx, dy) in cells.iter().copied() {
        let px = (dx - min_dx) as u16;
        let py = (dy - min_dy) as u16;
        let r = Rect {
            x: inner.x + off_x + px * NEXT_MINI_CELL_W,
            y: inner.y + off_y + py * NEXT_MINI_CELL_H,
            width: NEXT_MINI_CELL_W,
            height: NEXT_MINI_CELL_H,
        };
        let p = Paragraph::new("██").style(Style::default().fg(color).bg(color));
        p.render(r, frame.buffer_mut());
    }
}

fn piece_color_static(state: &GameState, kind: TetrominoKind) -> Color {
    state.theme.sand_color(kind.color_index(state.high_color))
}

/// Draw a row of 6 coloured blocks (sand palette).
fn draw_colour_strip(frame: &mut Frame, state: &GameState, area: Rect) {
    let block_w = (area.width / 6).max(1);
    for i in 0..6u8 {
        let r = Rect {
            x: area.x + (i as u16) * block_w,
            y: area.y,
            width: block_w,
            height: area.height.min(1),
        };
        let c = state.theme.sand_color(i);
        let p = Paragraph::new("█").style(Style::default().fg(c).bg(c));
        p.render(r, frame.buffer_mut());
    }
}

pub fn draw_quit_menu(frame: &mut Frame, state: &GameState, selected: crate::app::QuitOption) {
    let area = frame.area();
    let qw = 24;
    let qh = 8;
    let quit_rect = Rect {
        x: area.x + area.width.saturating_sub(qw) / 2,
        y: area.y + area.height.saturating_sub(qh) / 2,
        width: qw,
        height: qh,
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(state.theme.title))
        .title(" Quit? ");
    
    // Clear background
    for y in quit_rect.y..quit_rect.y + quit_rect.height {
        for x in quit_rect.x..quit_rect.x + quit_rect.width {
            frame.buffer_mut()[(x, y)].set_style(Style::default().bg(state.theme.bg));
        }
    }

    let inner = block.inner(quit_rect);
    block.render(quit_rect, frame.buffer_mut());

    let options = [
        (crate::app::QuitOption::Resume, " Resume "),
        (crate::app::QuitOption::MainMenu, " Main Menu "),
        (crate::app::QuitOption::Exit, " Exit "),
    ];

    for (i, (opt, label)) in options.iter().enumerate() {
        let style = if *opt == selected {
            Style::default().fg(state.theme.bg).bg(state.theme.title).bold()
        } else {
            Style::default().fg(state.theme.title)
        };
        let rx = inner.x + (inner.width.saturating_sub(label.len() as u16)) / 2;
        let ry = inner.y + 1 + i as u16 * 2;
        frame.buffer_mut().set_string(rx, ry, label, style);
    }
}
