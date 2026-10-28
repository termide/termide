//! Image panel for native graphics rendering.
//!
//! Uses ratatui-image to render images directly to the parent terminal
//! via its graphics protocol (Kitty, Sixel, iTerm2, or halfblocks fallback).
//!
//! The image opens fitted to the panel, aspect ratio preserved and centred.
//! `+`/`-` (or the mouse wheel) zoom around the centre of the visible part,
//! the arrow keys pan a zoomed image and `0` returns to the fitted view.

pub mod inline;
mod view;

use std::any::Any;
use std::path::{Path, PathBuf};

use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};
use image::{imageops::FilterType, DynamicImage};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    prelude::Widget,
    style::{Color, Style},
    widgets::Paragraph,
};
use ratatui_image::{picker::Picker, protocol::StatefulProtocol, Resize, StatefulImage};

use termide_core::{
    CommandResult, Config, Panel, PanelCommand, PanelEvent, PanelState, RenderContext, SegmentKind,
    StatusSegment, Theme, WidthPreference,
};

use view::Viewport;

/// Image panel for displaying images using terminal graphics protocols.
pub struct ImagePanel {
    /// Path to the image file
    file_path: PathBuf,
    /// Display title (filename)
    title: String,
    /// Graphics protocol picker (detects best available protocol)
    picker: Option<Picker>,
    /// Decoded image; crops of it are handed to the protocol as the view moves.
    source: Option<DynamicImage>,
    /// Encoded protocol for the source window it was built from `(x, y, w, h)`.
    /// Rebuilt only when zoom or panning change that window; the protocol
    /// itself re-encodes when the panel area changes.
    encoded: Option<((u32, u32, u32, u32), StatefulProtocol)>,
    /// Zoom and pan state.
    view: Viewport,
    /// Area of the last render, the frame zoom and pan are anchored to.
    last_area: Rect,
    /// Error message if image loading failed
    error: Option<String>,
}

impl ImagePanel {
    /// Create a new image panel for the given file path.
    pub fn new(path: PathBuf) -> Result<Self> {
        let t = termide_i18n::t();
        let title = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(t.panel_image())
            .to_string();

        let mut panel = Self {
            file_path: path.clone(),
            title,
            picker: None,
            source: None,
            encoded: None,
            view: Viewport::default(),
            last_area: Rect::default(),
            error: None,
        };

        // Initialize picker and load image
        panel.load_image(&path);

        Ok(panel)
    }

    /// Check if graphics protocol is available in the current terminal.
    ///
    /// The parent terminal is asked once per process, through
    /// [`inline::Graphics::detect`]: each query leaves a thread reading
    /// stdin when the answer is late, which then takes termide's keys.
    pub fn graphics_available() -> bool {
        inline::Graphics::detect().is_some()
    }

    /// Update the displayed image to a new path.
    pub fn set_image(&mut self, path: PathBuf) {
        self.file_path = path.clone();
        let t = termide_i18n::t();
        self.title = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(t.panel_image())
            .to_string();
        self.view.reset();
        self.load_image(&path);
    }

    /// Load image from file path.
    fn load_image(&mut self, path: &Path) {
        // Initialize picker if not already done
        if self.picker.is_none() {
            match inline::Graphics::detect() {
                Some(graphics) => self.picker = Some(graphics.into_picker()),
                None => {
                    let t = termide_i18n::t();
                    self.error =
                        Some(t.image_error_fmt("the terminal did not answer the graphics query"));
                    return;
                }
            }
        }

        // Load and decode image
        match image::open(path) {
            Ok(dyn_img) => {
                self.source = Some(dyn_img);
                self.encoded = None;
                self.error = None;
            }
            Err(e) => {
                let t = termide_i18n::t();
                self.error = Some(t.image_error_fmt(&e.to_string()));
            }
        }
    }

    /// Source image size in pixels.
    fn dims(&self) -> Option<(u32, u32)> {
        self.source.as_ref().map(|img| (img.width(), img.height()))
    }

    /// Cell size in pixels reported by the terminal.
    fn font(&self) -> (u16, u16) {
        self.picker
            .as_ref()
            .map(|p| p.font_size())
            .unwrap_or((1, 1))
    }

    /// Zoom by `steps` around the centre of the visible window.
    fn zoom(&mut self, steps: i32) -> Vec<PanelEvent> {
        let Some(dims) = self.dims() else {
            return vec![];
        };
        let before = self.view;
        self.view.zoom_by(steps, dims, self.last_area, self.font());
        if self.view == before {
            return vec![];
        }
        vec![PanelEvent::NeedsRedraw]
    }

    /// Pan the visible window; no-op while the whole image is on screen.
    fn pan(&mut self, dx: i32, dy: i32) -> Vec<PanelEvent> {
        let Some(dims) = self.dims() else {
            return vec![];
        };
        let before = self.view;
        self.view.pan(dx, dy, dims, self.last_area, self.font());
        if self.view == before {
            return vec![];
        }
        vec![PanelEvent::NeedsRedraw]
    }

    /// Back to the fitted view.
    fn fit(&mut self) -> Vec<PanelEvent> {
        if self.view.is_fit() {
            return vec![];
        }
        self.view.reset();
        vec![PanelEvent::NeedsRedraw]
    }
}

impl Panel for ImagePanel {
    fn name(&self) -> &'static str {
        "image"
    }

    fn width_preference(&self) -> WidthPreference {
        WidthPreference::PreferWide
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn prepare_render(&mut self, _theme: &Theme, _config: &std::sync::Arc<Config>) {
        // No preparation needed
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, _ctx: &RenderContext) {
        // Accordion already draws border with title, render directly to area
        self.last_area = area;

        // If there's an error, display it
        if let Some(ref error) = self.error {
            let error_text = Paragraph::new(error.as_str()).style(Style::default().fg(Color::Red));
            error_text.render(area, buf);
            return;
        }

        let (Some(picker), Some(source)) = (self.picker.as_ref(), self.source.as_ref()) else {
            return;
        };
        let dims = (source.width(), source.height());
        let Some(layout) = self.view.layout(dims, area, picker.font_size()) else {
            return;
        };

        // Hand the protocol only the visible window; it scales that to the
        // centred rect, so the rest of the area stays free of graphics.
        if self.encoded.as_ref().map(|(crop, _)| *crop) != Some(layout.crop) {
            let (x, y, w, h) = layout.crop;
            let window = if (x, y, w, h) == (0, 0, dims.0, dims.1) {
                source.clone()
            } else {
                source.crop_imm(x, y, w, h)
            };
            self.encoded = Some((layout.crop, picker.new_resize_protocol(window)));
        }

        // Crisp pixels once the image is magnified, smooth resampling otherwise.
        let filter = if layout.percent >= 200 {
            FilterType::Nearest
        } else {
            FilterType::Triangle
        };
        if let Some((_, state)) = self.encoded.as_mut() {
            let widget = StatefulImage::default().resize(Resize::Scale(Some(filter)));
            ratatui::prelude::StatefulWidget::render(widget, layout.rect, buf, state);
        }
    }

    fn handle_key(&mut self, chord: termide_core::KeyChord) -> Vec<PanelEvent> {
        // No text input here: every key is a shortcut, matched on the
        // layout-normalized form so it works on a Cyrillic layout too.
        let key = chord.canonical;
        if key.code == KeyCode::Char('q') {
            return vec![PanelEvent::ClosePanel];
        }
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return vec![];
        }
        match key.code {
            // `=` is the unshifted `+` on a US layout.
            KeyCode::Char('+') | KeyCode::Char('=') => self.zoom(1),
            KeyCode::Char('-') => self.zoom(-1),
            KeyCode::Char('0') => self.fit(),
            KeyCode::Left => self.pan(-1, 0),
            KeyCode::Right => self.pan(1, 0),
            KeyCode::Up => self.pan(0, -1),
            KeyCode::Down => self.pan(0, 1),
            _ => vec![],
        }
    }

    fn handle_scroll(&mut self, delta: i32, _panel_area: Rect) -> Vec<PanelEvent> {
        // Wheel up zooms in, wheel down zooms out; one step per batch.
        match delta.signum() {
            -1 => self.zoom(1),
            1 => self.zoom(-1),
            _ => vec![],
        }
    }

    fn status_segments(&self) -> Vec<StatusSegment> {
        let (Some(dims), Some(picker)) = (self.dims(), self.picker.as_ref()) else {
            return vec![];
        };
        let Some(layout) = self.view.layout(dims, self.last_area, picker.font_size()) else {
            return vec![];
        };
        let fit_kind = if self.view.is_fit() {
            SegmentKind::Inactive
        } else {
            SegmentKind::Active
        };
        vec![
            StatusSegment::new(" ", SegmentKind::Label),
            StatusSegment::new(format!("{}×{}", dims.0, dims.1), SegmentKind::Value),
            StatusSegment::new(" │ ", SegmentKind::Label),
            StatusSegment::new("Zoom: ", SegmentKind::Label),
            StatusSegment::new(format!("{}%", layout.percent), SegmentKind::Value),
            StatusSegment::new(" ", SegmentKind::Label),
            StatusSegment::clickable("[−]", SegmentKind::Active, "zoom_out"),
            StatusSegment::new(" ", SegmentKind::Label),
            StatusSegment::clickable("[+]", SegmentKind::Active, "zoom_in"),
            StatusSegment::new(" ", SegmentKind::Label),
            StatusSegment::clickable("Fit", fit_kind, "zoom_fit"),
        ]
    }

    fn handle_status_action(&mut self, action: &str) -> Vec<PanelEvent> {
        match action {
            "zoom_in" => self.zoom(1),
            "zoom_out" => self.zoom(-1),
            "zoom_fit" => self.fit(),
            _ => vec![],
        }
    }

    fn handle_command(&mut self, cmd: PanelCommand<'_>) -> CommandResult {
        match cmd {
            PanelCommand::Reload => {
                self.load_image(&self.file_path.clone());
                CommandResult::NeedsRedraw(true)
            }
            _ => CommandResult::None,
        }
    }

    fn to_state(&self, _project_dir: &Path) -> Option<PanelState> {
        Some(PanelState::Image {
            path: self.file_path.clone(),
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn get_working_directory(&self) -> Option<PathBuf> {
        self.file_path.parent().map(|p| p.to_path_buf())
    }
}
