//! GPUI 输入框和按钮。输入框自己处理按键，这样界面不用再套一层 WebView。

use open_gpui::prelude::*;
use open_gpui::{
    div, px, rgb, App, Context, CursorStyle, Div, ElementId, EventEmitter, FocusHandle, Focusable,
    IntoElement, KeyDownEvent, MouseButton, Render, SharedString, Stateful, Window,
};

#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: u32,
    pub sidebar: u32,
    pub panel: u32,
    pub deep: u32,
    pub raised: u32,
    pub line: u32,
    pub text: u32,
    pub muted: u32,
    pub accent: u32,
    pub danger: u32,
    pub success: u32,
    pub warning: u32,
    pub info: u32,
    pub track: u32,
    pub selected: u32,
    #[allow(dead_code)]
    pub hover: u32,
}

pub fn palette(theme: &str) -> Palette {
    if theme == "light" {
        Palette {
            bg: 0xf5f7fb,
            sidebar: 0xffffff,
            panel: 0xffffff,
            deep: 0xf7f8fc,
            raised: 0xeef1f7,
            line: 0xdfe3ec,
            text: 0x202534,
            muted: 0x70798c,
            accent: 0x5069ed,
            danger: 0xb03048,
            success: 0x2d8a5e,
            warning: 0x8a6328,
            info: 0x425acb,
            track: 0xe4e8f1,
            selected: 0xe9edff,
            hover: 0xf3f5fa,
        }
    } else {
        Palette {
            bg: 0x0e1016,
            sidebar: 0x10131b,
            panel: 0x171a23,
            deep: 0x13161e,
            raised: 0x20242f,
            line: 0x292e3b,
            text: 0xf0f2f6,
            muted: 0x8e96a8,
            accent: 0x5b74ff,
            danger: 0xe05b70,
            success: 0x4bc486,
            warning: 0xdd9b3d,
            info: 0x7ea1ff,
            track: 0x0d1016,
            selected: 0x202638,
            hover: 0x1b1f29,
        }
    }
}

pub enum FieldEvent {
    Submit,
}

pub struct Field {
    pub focus_handle: FocusHandle,
    id: SharedString,
    text: String,
    cursor: usize,
    select_all: bool,
    placeholder: SharedString,
    masked: bool,
    text_color: u32,
    muted_color: u32,
    bg: u32,
    border: u32,
    accent: u32,
}

impl Field {
    pub fn new(
        cx: &mut Context<Self>,
        id: impl Into<SharedString>,
        placeholder: impl Into<SharedString>,
        masked: bool,
    ) -> Self {
        let colors = palette("dark");
        Self {
            focus_handle: cx.focus_handle(),
            id: id.into(),
            text: String::new(),
            cursor: 0,
            select_all: false,
            placeholder: placeholder.into(),
            masked,
            text_color: colors.text,
            muted_color: colors.muted,
            bg: colors.deep,
            border: colors.line,
            accent: colors.accent,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn set_text(&mut self, text: impl AsRef<str>, cx: &mut Context<Self>) {
        self.text = text.as_ref().to_string();
        self.cursor = self.text.len();
        self.select_all = false;
        cx.notify();
    }

    pub fn set_palette(&mut self, palette: Palette, cx: &mut Context<Self>) {
        self.text_color = palette.text;
        self.muted_color = palette.muted;
        self.bg = palette.deep;
        self.border = palette.line;
        self.accent = palette.accent;
        cx.notify();
    }

    fn insert(&mut self, text: &str) {
        if self.select_all {
            self.text.clear();
            self.cursor = 0;
            self.select_all = false;
        }
        let cursor = self.cursor.min(self.text.len());
        self.text.insert_str(cursor, text);
        self.cursor = cursor + text.len();
    }

    fn backspace(&mut self) {
        if self.select_all {
            self.text.clear();
            self.cursor = 0;
            self.select_all = false;
            return;
        }
        if self.cursor == 0 {
            return;
        }
        let previous = previous_boundary(&self.text, self.cursor);
        self.text.replace_range(previous..self.cursor, "");
        self.cursor = previous;
    }

    fn delete_forward(&mut self) {
        if self.select_all {
            self.text.clear();
            self.cursor = 0;
            self.select_all = false;
            return;
        }
        if self.cursor >= self.text.len() {
            return;
        }
        let next = next_boundary(&self.text, self.cursor);
        self.text.replace_range(self.cursor..next, "");
    }

    fn on_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let key = keystroke.key.as_str();
        let command = keystroke.modifiers.platform || keystroke.modifiers.control;
        if command && key == "a" {
            self.select_all = !self.text.is_empty();
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if command && key == "c" {
            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                let _ = clipboard.set_text(self.text.clone());
            }
            cx.stop_propagation();
            return;
        }
        if command && (key == "v" || key == "x") {
            if key == "x" {
                if let Ok(mut clipboard) = arboard::Clipboard::new() {
                    let _ = clipboard.set_text(self.text.clone());
                }
                self.text.clear();
                self.cursor = 0;
                self.select_all = false;
            } else if let Ok(mut clipboard) = arboard::Clipboard::new() {
                if let Ok(text) = clipboard.get_text() {
                    let text = text.replace(['\n', '\r'], "");
                    self.insert(&text);
                }
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if key == "enter" {
            cx.emit(FieldEvent::Submit);
            cx.stop_propagation();
            return;
        }
        if key == "backspace" {
            self.backspace();
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if key == "delete" {
            self.delete_forward();
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if key == "left" {
            self.select_all = false;
            self.cursor = previous_boundary(&self.text, self.cursor);
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if key == "right" {
            self.select_all = false;
            self.cursor = next_boundary(&self.text, self.cursor);
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if key == "home" {
            self.select_all = false;
            self.cursor = 0;
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if key == "end" {
            self.select_all = false;
            self.cursor = self.text.len();
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if command || keystroke.modifiers.alt || key == "escape" || key == "tab" {
            return;
        }
        if let Some(text) = keystroke.key_char.as_deref() {
            if !text.chars().any(char::is_control) {
                self.insert(text);
                cx.stop_propagation();
                cx.notify();
            }
        }
    }
}

impl EventEmitter<FieldEvent> for Field {}

impl Focusable for Field {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Field {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = window
            .focused(cx)
            .is_some_and(|handle| handle == self.focus_handle);
        let border = if focused { self.accent } else { self.border };
        let (label, placeholder) = display_text(self, focused);
        let color = if placeholder {
            self.muted_color
        } else {
            self.text_color
        };
        div()
            .id(self.id.clone())
            .w_full()
            .h(px(34.))
            .px(px(10.))
            .flex()
            .items_center()
            .rounded(px(7.))
            .bg(rgb(self.bg))
            .border_1()
            .border_color(rgb(border))
            .text_size(px(13.))
            .text_color(rgb(color))
            .cursor(CursorStyle::IBeam)
            .track_focus(&self.focus_handle)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.cursor = this.text.len();
                    this.select_all = false;
                    window.focus(&this.focus_handle, cx);
                    cx.notify();
                }),
            )
            .on_key_down(cx.listener(|this, event, _, cx| this.on_key(event, cx)))
            .child(label)
    }
}

fn display_text(field: &Field, focused: bool) -> (String, bool) {
    if field.text.is_empty() {
        let placeholder = if focused {
            format!("|{}", field.placeholder)
        } else {
            field.placeholder.to_string()
        };
        return (placeholder, true);
    }
    let shown: String = if field.masked {
        "•".repeat(field.text.chars().count())
    } else {
        field.text.clone()
    };
    if !focused {
        return (shown, false);
    }
    if field.select_all {
        return (format!("[{shown}]"), false);
    }
    let cursor_chars = field.text[..field.cursor].chars().count();
    let mut painted = String::new();
    for (index, ch) in shown.chars().enumerate() {
        if index == cursor_chars {
            painted.push('|');
        }
        painted.push(ch);
    }
    if cursor_chars >= shown.chars().count() {
        painted.push('|');
    }
    (painted, false)
}

fn previous_boundary(text: &str, cursor: usize) -> usize {
    if cursor == 0 {
        return 0;
    }
    let mut index = cursor - 1;
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn next_boundary(text: &str, cursor: usize) -> usize {
    if cursor >= text.len() {
        return text.len();
    }
    let mut index = cursor + 1;
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

pub fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    bg: u32,
    fg: u32,
    enabled: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(32.))
        .px(px(12.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(7.))
        .text_size(px(13.))
        .bg(rgb(if enabled { bg } else { 0x3a4050 }))
        .text_color(rgb(if enabled { fg } else { 0x9aa3b5 }))
        .when(enabled, |el| el.cursor(CursorStyle::PointingHand))
        .child(label.into())
}
