//! App: terminal init, main loop, tick and key handling.

use crate::game::GameState;
use crate::input::{key_to_action, Action};
use crate::{Args, GameConfig};
use crate::theme::Theme;
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::DefaultTerminal;
use std::time::{Duration, Instant};
use tachyonfx::Effect;

/// DAS (Delayed Auto-Shift): delay before movement starts repeating when you hold a key.
const REPEAT_DELAY_MS: u64 = 170;
/// ARR (Auto-Repeat Rate): time between repeated moves while holding. 50 ms ≈ 20 moves/sec.
const REPEAT_INTERVAL_MS: u64 = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Menu,
    Playing,
    GameOver,
    QuitMenu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitOption {
    Resume,
    MainMenu,
    Exit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameOverReason {
    StackOverflow,
    TimeUp,
    ClearedN,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuTab {
    Difficulty,
    Mode,
    Start,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuState {
    pub current_tab: MenuTab,
    pub selected_difficulty: crate::Difficulty,
    pub selected_mode: crate::GameMode,
    pub animation_start: Instant,
    pub ratman_typed: String,
    pub ratman_unlocked: bool,
}

impl Default for MenuState {
    fn default() -> Self {
        Self {
            current_tab: MenuTab::Difficulty,
            selected_difficulty: crate::Difficulty::Easy,
            selected_mode: crate::GameMode::Endless,
            animation_start: Instant::now(),
            ratman_typed: String::new(),
            ratman_unlocked: false,
        }
    }
}

pub struct App {
    args: Args,
    config: GameConfig,
    theme: Theme,
    /// Playfield size clamped to terminal so board + border fit on screen.
    effective_playfield_width: u16,
    effective_playfield_height: u16,
    state: GameState,
    screen: Screen,
    paused: bool,
    game_start: Instant,
    game_over_reason: Option<GameOverReason>,
    last_tick: Instant,
    /// Base tick rate (Hz) for level-based speed when not relaxed.
    base_tick_rate: f64,
    repeat_state: Option<(Action, Instant)>,
    last_repeat_fire: Option<Instant>,
    last_input_time: Instant,
    line_clear_started: Option<Instant>,
    /// TachyonFX fade effect for line-clear (created when animation starts).
    line_clear_effect: Option<Effect>,
    /// Last time we processed the line-clear effect (for delta).
    line_clear_effect_process_time: Option<Instant>,
    menu_state: MenuState,
    quit_selected: QuitOption,
    high_score_endless: u32,
    high_score_timed: u32,
    /// Playfield size from current terminal when on menu (zoom out = bigger). Used when starting from menu; during play size is fixed.
    menu_playfield_width: u16,
    menu_playfield_height: u16,
}

fn default_tick_rate_for_difficulty(d: crate::Difficulty) -> f64 {
    match d {
        crate::Difficulty::Easy => 30.0,
        crate::Difficulty::Medium => 50.0,
        crate::Difficulty::Hard => 90.0,
    }
}

impl App {
    pub fn new(args: Args, config: GameConfig, theme: Theme) -> Result<Self> {
        let width = crate::effective_playfield_width(args.difficulty, args.width);
        let height = args.height;
        let state = GameState::new(theme.clone(), width, height, &config);
        let tick_rate = if args.tick_rate == 18.0 {
            default_tick_rate_for_difficulty(args.difficulty)
        } else {
            args.tick_rate
        };
        let screen = if args.no_menu {
            Screen::Playing
        } else {
            Screen::Menu
        };
        let now = Instant::now();
        Ok(Self {
            args,
            config: config.clone(),
            theme,
            effective_playfield_width: width,
            effective_playfield_height: height,
            state,
            screen,
            paused: false,
            game_start: now,
            game_over_reason: None,
            last_tick: now,
            base_tick_rate: tick_rate,
            repeat_state: None,
            last_repeat_fire: None,
            last_input_time: now,
            line_clear_started: None,
            line_clear_effect: None,
            line_clear_effect_process_time: None,
            menu_state: MenuState::default(),
            quit_selected: QuitOption::Resume,
            high_score_endless: 0,
            high_score_timed: 0,
            menu_playfield_width: width,
            menu_playfield_height: height,
        })
    }

    fn reset_game(&mut self) {
        let width = self.effective_playfield_width;
        let height = self.effective_playfield_height;
        let now = Instant::now();
        let old_menu_state = self.menu_state.clone();
        
        // Recalculate base tick rate according to current difficulty
        self.base_tick_rate = default_tick_rate_for_difficulty(self.args.difficulty);
        
        self.state = GameState::new(self.theme.clone(), width, height, &self.config);
        self.screen = Screen::Playing;
        self.paused = false;
        self.game_start = now;
        self.game_over_reason = None;
        self.last_tick = now;
        self.repeat_state = None;
        self.last_repeat_fire = None;
        self.line_clear_started = None;
        self.line_clear_effect = None;
        self.line_clear_effect_process_time = None;
        self.menu_state = old_menu_state; // Keep the ratman status!
        
        if self.menu_state.ratman_unlocked {
            self.args.high_color = true;
            self.state = GameState::new(self.theme.clone(), self.effective_playfield_width, self.effective_playfield_height, &self.config);
            self.state.high_color = true;
            self.base_tick_rate *= 1.5; // Ratman is extra fast
        }
    }

    fn apply_action(&mut self, action: Action, now: Instant) {
        match action {
            Action::Quit => {}
            Action::Pause => {}
            Action::MoveLeft => self.state.move_left(now),
            Action::MoveRight => self.state.move_right(now),
            Action::RotateCw => self.state.rotate_cw(now),
            Action::RotateCcw => self.state.rotate_ccw(now),
            Action::SoftDrop => self.state.soft_drop(now),
            Action::HardDrop => {
                self.state.hard_drop(now);
                self.repeat_state = None;
            }
            Action::None => {}
        }
    }

    fn tick_repeat(&mut self) {
        let now = Instant::now();
        let (action, first) = match self.repeat_state {
            Some(s) => s,
            None => return,
        };
        if action == Action::Quit || action == Action::HardDrop || action == Action::Pause || action == Action::None {
            return;
        }
        
        // Removed safety fallback that assumed sticky keys after 100ms;
        // now relying on KeyEventKind::Release and standard DAS/ARR logic.

        if first.elapsed() < Duration::from_millis(REPEAT_DELAY_MS) {
            return;
        }
        let next = self.last_repeat_fire.unwrap_or(first) + Duration::from_millis(REPEAT_INTERVAL_MS);
        if now >= next {
            self.apply_action(action, now);
            if matches!(
                action,
                Action::MoveLeft | Action::MoveRight | Action::RotateCw | Action::RotateCcw
            ) {
                self.state.on_move_or_rotate(now);
            }
            self.last_repeat_fire = Some(now);
        }
    }

    pub fn run(&mut self) -> Result<()> {
        use crossterm::{
            execute,
            terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen, size},
            event::{PushKeyboardEnhancementFlags, PopKeyboardEnhancementFlags, KeyboardEnhancementFlags},
        };

        enable_raw_mode()?;
        let mut stdout = std::io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        
        // Attempt to enable enhanced keyboard for Release events
        let _ = execute!(stdout, PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::REPORT_EVENT_TYPES
        ));

        let mut terminal = ratatui::DefaultTerminal::new(ratatui::backend::CrosstermBackend::new(stdout))?;

        // Size playfield to fit terminal (no squeeze); respect --width/--height when they fit
        let (term_cols, term_rows) = size()?;
        let (fit_w, fit_h) = crate::ui::playfield_size_for_terminal_clamped(term_cols, term_rows);
        let requested_w = crate::effective_playfield_width(self.args.difficulty, self.args.width);
        let requested_h = self.args.height;
        self.effective_playfield_width = requested_w.min(fit_w).max(1);
        self.effective_playfield_height = requested_h.min(fit_h).max(1);
        self.menu_playfield_width = self.effective_playfield_width;
        self.menu_playfield_height = self.effective_playfield_height;
        let need_resize = self.state.playfield.width != self.effective_playfield_width as usize
            || self.state.playfield.height != self.effective_playfield_height as usize;
        if need_resize {
            self.state = GameState::new(
                self.theme.clone(),
                self.effective_playfield_width,
                self.effective_playfield_height,
                &self.config,
            );
        }

        let result = self.run_loop(&mut terminal);

        // Restore
        let _ = execute!(std::io::stdout(), PopKeyboardEnhancementFlags);
        execute!(std::io::stdout(), LeaveAlternateScreen)?;
        disable_raw_mode()?;

        result
    }

    fn run_loop(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        loop {
            let now = Instant::now();
            if self.screen == Screen::Menu {
                let (c, r) = crossterm::terminal::size().unwrap_or((80, 24));
                let (w, h) = crate::ui::playfield_size_for_terminal_clamped(c, r);
                self.menu_playfield_width = w;
                self.menu_playfield_height = h;
            }
            let menu_size = (self.screen == Screen::Menu).then(|| (self.menu_playfield_width, self.menu_playfield_height));
            terminal.draw(|f| {
                crate::ui::draw(
                    f,
                    self.screen,
                    &self.state,
                    self.paused,
                    self.game_over_reason,
                    self.args.mode,
                    self.args.clear_lines,
                    self.args.time_limit,
                    self.game_start,
                    f.area(),
                    &mut self.line_clear_effect,
                    &mut self.line_clear_effect_process_time,
                    &mut self.menu_state,
                    now,
                    self.args.no_animation,
                    if self.screen == Screen::QuitMenu { Some(self.quit_selected) } else { None },
                    menu_size,
                )
            })?;

            if self.state.line_clear_in_progress
                && !self.args.no_animation
                && self.line_clear_effect.as_ref().is_some_and(|e| e.done())
            {
                self.state.finish_line_clear();
                self.line_clear_effect = None;
                self.line_clear_effect_process_time = None;
                self.line_clear_started = None;
            }

            let mut rate = if self.args.relaxed {
                self.base_tick_rate
            } else {
                self.base_tick_rate * (1.0 + (self.state.level.saturating_sub(1) as f64) * 0.1)
            };
            
            if self.menu_state.ratman_unlocked {
                rate *= 2.0;
            }

            let tick_interval = Duration::from_secs_f64(1.0 / rate);
            
            // Limit event polling to hit ~60 FPS rendering (16ms)
            let frame_duration = Duration::from_millis(16);
            let loop_elapsed = now.elapsed();
            let timeout = frame_duration.saturating_sub(loop_elapsed);

            // Tick popups
            self.state.tick_popups(16);

            // Timed mode check
            if self.screen == Screen::Playing && self.args.mode == crate::GameMode::Timed {
                let elapsed = now.duration_since(self.game_start).as_secs();
                if elapsed >= self.args.time_limit as u64 {
                    self.screen = Screen::GameOver;
                    self.game_over_reason = Some(GameOverReason::TimeUp);
                }
            }
            
            // High score update
            match self.args.mode {
                crate::GameMode::Endless => if self.state.score > self.high_score_endless { self.high_score_endless = self.state.score; },
                crate::GameMode::Timed => if self.state.score > self.high_score_timed { self.high_score_timed = self.state.score; },
                _ => {}
            }

            if event::poll(timeout)? {
                while event::poll(Duration::ZERO)? {
                    if let Event::Key(key) = event::read()? {
                        let action = key_to_action(key);
                        self.last_input_time = Instant::now();
                        
                        // Ignore OS repeats and only process first Press.
                        // Filter out redundant OS presses if we're already repeating that action ourselves.
                        if key.kind != KeyEventKind::Press {
                            if key.kind == KeyEventKind::Release
                                && self.repeat_state.map(|(a, _)| a) == Some(action)
                            {
                                self.repeat_state = None;
                                self.last_repeat_fire = None;
                            }
                            continue;
                        }
                        
                        // If we are already repeating this action, ignore subsequent OS Press events
                        if self.repeat_state.map(|(a, _)| a) == Some(action) {
                            continue;
                        }

                        match self.screen {
                            Screen::Menu => {
                                match action {
                                    Action::Quit => return Ok(()),
                                    Action::MoveLeft => {
                                        match self.menu_state.current_tab {
                                            MenuTab::Difficulty => {
                                                self.menu_state.selected_difficulty = match self.menu_state.selected_difficulty {
                                                    crate::Difficulty::Easy => crate::Difficulty::Hard,
                                                    crate::Difficulty::Medium => crate::Difficulty::Easy,
                                                    crate::Difficulty::Hard => crate::Difficulty::Medium,
                                                };
                                            }
                                            MenuTab::Mode => {
                                                self.menu_state.selected_mode = match self.menu_state.selected_mode {
                                                    crate::GameMode::Endless => crate::GameMode::Clear,
                                                    crate::GameMode::Timed => crate::GameMode::Endless,
                                                    crate::GameMode::Clear => crate::GameMode::Timed,
                                                };
                                            }
                                            _ => {}
                                        }
                                    }
                                    Action::MoveRight => {
                                        match self.menu_state.current_tab {
                                            MenuTab::Difficulty => {
                                                self.menu_state.selected_difficulty = match self.menu_state.selected_difficulty {
                                                    crate::Difficulty::Easy => crate::Difficulty::Medium,
                                                    crate::Difficulty::Medium => crate::Difficulty::Hard,
                                                    crate::Difficulty::Hard => crate::Difficulty::Easy,
                                                };
                                            }
                                            MenuTab::Mode => {
                                                self.menu_state.selected_mode = match self.menu_state.selected_mode {
                                                    crate::GameMode::Endless => crate::GameMode::Timed,
                                                    crate::GameMode::Timed => crate::GameMode::Clear,
                                                    crate::GameMode::Clear => crate::GameMode::Endless,
                                                };
                                            }
                                            _ => {}
                                        }
                                    }
                                    Action::SoftDrop => {
                                        self.menu_state.current_tab = match self.menu_state.current_tab {
                                            MenuTab::Difficulty => MenuTab::Mode,
                                            MenuTab::Mode => MenuTab::Start,
                                            MenuTab::Start => MenuTab::Difficulty,
                                        };
                                    }
                                    Action::RotateCw | Action::RotateCcw => {
                                        self.menu_state.current_tab = match self.menu_state.current_tab {
                                            MenuTab::Difficulty => MenuTab::Start,
                                            MenuTab::Mode => MenuTab::Difficulty,
                                            MenuTab::Start => MenuTab::Mode,
                                        };
                                    }
                                    Action::HardDrop => {
                                            if self.menu_state.current_tab == MenuTab::Start {
                                            self.args.difficulty = self.menu_state.selected_difficulty;
                                            self.args.mode = self.menu_state.selected_mode;
                                            self.config.difficulty = self.args.difficulty;
                                            self.effective_playfield_width = self.menu_playfield_width;
                                            self.effective_playfield_height = self.menu_playfield_height;
                                            self.reset_game();
                                        } else {
                                            self.menu_state.current_tab = MenuTab::Start;
                                        }
                                    }
                                    _ => {
                                        if let KeyCode::Char(c) = key.code {
                                            self.menu_state.ratman_typed.push(c);
                                            if "Ratman".starts_with(&self.menu_state.ratman_typed) {
                                                if self.menu_state.ratman_typed == "Ratman" {
                                                    self.menu_state.ratman_unlocked = true;
                                                }
                                            } else {
                                                self.menu_state.ratman_typed = c.to_string();
                                                if c == 'R' {
                                                    // Start over maybe?
                                                } else {
                                                    self.menu_state.ratman_typed.clear();
                                                }
                                            }
                                        }

                                        if key.code == KeyCode::Enter {
                                            if self.menu_state.current_tab == MenuTab::Start {
                                                self.args.difficulty = self.menu_state.selected_difficulty;
                                                self.args.mode = self.menu_state.selected_mode;
                                                self.config.difficulty = self.args.difficulty;
                                                self.effective_playfield_width = self.menu_playfield_width;
                                                self.effective_playfield_height = self.menu_playfield_height;
                                                self.reset_game();
                                            } else {
                                                self.menu_state.current_tab = MenuTab::Start;
                                            }
                                        }
                                    }
                                }
                            }
                            Screen::Playing => {
                                if self.paused {
                                    if action == Action::Pause { self.paused = false; }
                                    else if action == Action::Quit { 
                                        self.screen = Screen::QuitMenu;
                                        self.quit_selected = QuitOption::Resume;
                                    }
                                } else {
                                    if action == Action::Pause { self.paused = true; }
                                    else {
                                        self.apply_action(action, Instant::now());
                                        if matches!(action, Action::MoveLeft | Action::MoveRight | Action::RotateCw | Action::RotateCcw) {
                                            self.state.on_move_or_rotate(Instant::now());
                                        }
                                        let repeatable = matches!(action, Action::MoveLeft | Action::MoveRight | Action::SoftDrop);
                                        if repeatable {
                                            self.repeat_state = Some((action, Instant::now()));
                                            self.last_repeat_fire = None;
                                        }
                                        if action == Action::Quit { 
                                            self.screen = Screen::QuitMenu;
                                            self.quit_selected = QuitOption::Resume;
                                        }
                                        
                                        // If the action caused a lock, clear repeat state to prevent "input memory"
                                        if self.state.line_clear_in_progress || self.state.piece.is_none() {
                                            self.repeat_state = None;
                                        }
                                    }
                                }
                            }
                            Screen::QuitMenu => {
                                match action {
                                    Action::SoftDrop | Action::MoveRight => {
                                        // Cycle Down
                                        self.quit_selected = match self.quit_selected {
                                            QuitOption::Resume => QuitOption::MainMenu,
                                            QuitOption::MainMenu => QuitOption::Exit,
                                            QuitOption::Exit => QuitOption::Resume,
                                        };
                                    }
                                    Action::RotateCw | Action::RotateCcw | Action::MoveLeft => {
                                        // Cycle Up
                                        self.quit_selected = match self.quit_selected {
                                            QuitOption::Resume => QuitOption::Exit,
                                            QuitOption::MainMenu => QuitOption::Resume,
                                            QuitOption::Exit => QuitOption::MainMenu,
                                        };
                                    }
                                    Action::HardDrop => {
                                        match self.quit_selected {
                                            QuitOption::Resume => self.screen = Screen::Playing,
                                            QuitOption::MainMenu => self.screen = Screen::Menu,
                                            QuitOption::Exit => return Ok(()),
                                        }
                                    }
                                    Action::Pause | Action::Quit => {
                                        self.screen = Screen::Playing;
                                    }
                                    _ => {
                                        // If user hits Enter/Space directly via Action::HardDrop it confirm.
                                        // The SoftDrop (Down) and RotateCw (Up) are now mapped to cycling.
                                    }
                                }
                            }
                            Screen::GameOver => {
                                if action == Action::Quit { return Ok(()); }
                                if key.code == KeyCode::Char('r') || key.code == KeyCode::Char('R') {
                                    self.reset_game();
                                }
                            }
                        }
                    }
                }
            }

            if self.screen == Screen::Playing && !self.paused {
                self.tick_repeat();
                if self.last_tick.elapsed() >= tick_interval {
                    self.last_tick = Instant::now();
                    self.state.tick_gravity(Instant::now());

                    // --- DIFFICULTY-SYNCED SAND ---
                    // Physics now move at the same rate as gravity/logic.
                    // This makes "Easy" feel heavy and deliberate, and "Hard" fast but fair.
                    let steps = if self.menu_state.ratman_unlocked { 2 } else { 1 };
                    for _ in 0..steps {
                        self.state.tick_sand();
                    }
                }
                
                // Check for locking EVERY frame for maximum "snappiness"
                self.state.check_lock(Instant::now());

                // --- DYNAMIC CLEAR CHECK ---
                // Already handled inside state.tick_sand() or when piece locks.
                if self.state.game_over {
                    self.game_over_reason = Some(GameOverReason::StackOverflow);
                    self.screen = Screen::GameOver;
                } else if self.args.mode == crate::GameMode::Timed
                    && self.game_start.elapsed() >= Duration::from_secs(self.args.time_limit as u64)
                {
                    self.game_over_reason = Some(GameOverReason::TimeUp);
                    self.screen = Screen::GameOver;
                } else if self.args.mode == crate::GameMode::Clear
                    && self.state.lines_cleared >= self.args.clear_lines
                {
                    self.game_over_reason = Some(GameOverReason::ClearedN);
                    self.screen = Screen::GameOver;
                }
                if self.state.line_clear_in_progress && !self.state.line_clear_cells.is_empty() {
                    if self.args.no_animation {
                        self.state.finish_line_clear();
                        self.line_clear_started = None;
                        self.line_clear_effect = None;
                        self.line_clear_effect_process_time = None;
                    }
                }
            }
        }
    }
}
