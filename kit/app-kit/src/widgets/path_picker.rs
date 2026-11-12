//! Native file picking with an in-app fallback when the desktop has no picker.
use crate::host::{Host, Layer};
use crate::theme::{board, size, theme};
use crate::widgets::field::{FieldEvent, TextField};
use crate::widgets::kit::{self, Kind};
use gpui::{
    App, AppContext, Context, Entity, ExternalPaths, Focusable, PathPromptOptions, Render, Window,
    div, prelude::*, px,
};
use std::path::{Path, PathBuf};

const LAYER: &str = "path-picker";
type Picked = Box<dyn FnOnce(Vec<PathBuf>, &mut App)>;

/// Opens the native picker. A picker error opens a path field; cancellation
/// does nothing. The callback runs only when files have been selected.
pub fn pick<T: 'static>(
    host: Host,
    prompt: &'static str,
    window: &mut Window,
    cx: &mut Context<T>,
    selected: impl FnOnce(&mut T, Vec<PathBuf>, &mut Context<T>) + 'static,
) {
    let page = cx.weak_entity();
    let picked = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: false,
        multiple: true,
        prompt: Some(prompt.into()),
    });
    window
        .spawn(cx, async move |cx| {
            let answer = picked.await;
            let _ = cx.update(|window, cx| {
                let callback: Picked = Box::new(move |paths, cx| {
                    let _ = page.update(cx, |page, cx| selected(page, paths, cx));
                });
                match choice(answer) {
                    Choice::Selected(paths) => callback(paths, cx),
                    Choice::Cancelled => {}
                    Choice::Fallback => open(host, prompt, callback, window, cx),
                }
            });
        })
        .detach();
}

enum Choice {
    Selected(Vec<PathBuf>),
    Cancelled,
    Fallback,
}

fn choice<E, C>(answer: Result<Result<Option<Vec<PathBuf>>, E>, C>) -> Choice {
    match answer {
        Ok(Ok(Some(paths))) => Choice::Selected(paths),
        Ok(Ok(None)) => Choice::Cancelled,
        Ok(Err(_)) | Err(_) => Choice::Fallback,
    }
}

fn open(host: Host, prompt: &'static str, picked: Picked, window: &mut Window, cx: &mut App) {
    let opener = window.focused(cx);
    let view = cx.new(|cx| Picker::new(host.clone(), prompt, picked, window, cx));
    let focus = view.read(cx).field.focus_handle(cx);
    host.layer(
        Layer::new(LAYER, view)
            .focus(focus.clone())
            .on_dismiss(move |window, cx| {
                if let Some(opener) = &opener {
                    window.focus(opener, cx);
                }
            }),
        cx,
    );
    window.focus(&focus, cx);
}

struct Picker {
    host: Host,
    title: &'static str,
    field: Entity<TextField>,
    error: Option<String>,
    picked: Option<Picked>,
}

impl Picker {
    fn new(
        host: Host,
        title: &'static str,
        picked: Picked,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let field = cx.new(|cx| TextField::new(cx, "Path on this device"));
        cx.subscribe_in(
            &field,
            window,
            |picker: &mut Self, _, event, _, cx| match event {
                FieldEvent::Submit => picker.submit(cx),
                FieldEvent::Tab => picker.complete(cx),
                FieldEvent::Changed => {
                    picker.error = None;
                    cx.notify();
                }
                _ => {}
            },
        )
        .detach();
        Self {
            host,
            title,
            field,
            error: None,
            picked: Some(picked),
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let path = PathBuf::from(self.field.read(cx).text.trim());
        if !path.is_file() {
            self.error = Some("Enter a path to a file on this device.".into());
            cx.notify();
            return;
        }
        self.finish(vec![path], cx);
    }

    fn finish(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        let callback = self.picked.take();
        self.host.close_layer(LAYER, cx);
        if let Some(callback) = callback {
            callback(paths, cx);
        }
    }

    fn complete(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = complete(&self.field.read(cx).text) {
            self.field
                .update(cx, |field, cx| field.set(&path, false, cx));
        }
    }
}

/// Complete the final component, stopping at the common prefix of matches.
fn complete(input: &str) -> Option<String> {
    if input.is_empty() {
        return None;
    }
    let path = Path::new(input);
    let trailing = input.ends_with(std::path::MAIN_SEPARATOR);
    let (dir, prefix) = if trailing {
        (path, "")
    } else {
        (
            path.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
            path.file_name()?.to_str()?,
        )
    };
    let mut matches: Vec<_> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            name.starts_with(prefix)
                .then_some((name, entry.path().is_dir()))
        })
        .collect();
    matches.sort_by(|a, b| a.0.cmp(&b.0));
    let (first, is_dir) = matches.first()?;
    let mut end = first.len();
    for (name, _) in &matches[1..] {
        end = first[..end]
            .chars()
            .zip(name.chars())
            .take_while(|(a, b)| a == b)
            .map(|(c, _)| c.len_utf8())
            .sum();
    }
    let base = &input[..input.len() - prefix.len()];
    let mut result = format!("{base}{}", &first[..end]);
    if matches.len() == 1 && *is_dir {
        result.push(std::path::MAIN_SEPARATOR);
    }
    Some(result)
}

impl Render for Picker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        div()
            .size_full()
            .flex()
            .justify_center()
            .items_start()
            .pt(px(board::PAD))
            .on_drop(cx.listener(|picker, paths: &ExternalPaths, _, cx| {
                picker.finish(paths.paths().to_vec(), cx)
            }))
            .child(
                div()
                    .id("path-picker")
                    .debug_selector(|| "path-picker".into())
                    .w(px(size::KEYS))
                    .p(px(board::PAD))
                    .bg(t.surface)
                    .border_1()
                    .border_color(t.divider)
                    .flex()
                    .flex_col()
                    .gap(px(board::GAP))
                    .child(kit::label(&t, self.title))
                    .child(kit::note(
                        &t,
                        "Enter a file path, or drop files here. Tab completes.",
                        Some(t.dim),
                    ))
                    .child(
                        div()
                            .debug_selector(|| "path-picker-field".into())
                            .child(self.field.clone()),
                    )
                    .when_some(self.error.clone(), |d, error| {
                        d.child(kit::note(&t, error, Some(t.error)))
                    })
                    .child(
                        div()
                            .flex()
                            .gap(px(board::GAP))
                            .child(
                                kit::button(&t, "path-picker-select", "Select", Kind::Primary)
                                    .on_click(cx.listener(|picker, _, _, cx| picker.submit(cx))),
                            )
                            .child(
                                kit::button(&t, "path-picker-cancel", "Cancel", Kind::Quiet)
                                    .on_click(cx.listener(|picker, _, _, cx| {
                                        picker.host.close_layer(LAYER, cx)
                                    })),
                            ),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc};
    #[test]
    fn native_picker_selection_cancellation_and_failure() {
        let path = PathBuf::from("example.txt");
        assert!(
            matches!(choice::<(), ()>(Ok(Ok(Some(vec![path.clone()])))), Choice::Selected(paths) if paths == vec![path])
        );
        assert!(matches!(choice::<(), ()>(Ok(Ok(None))), Choice::Cancelled));
        assert!(matches!(choice::<(), ()>(Ok(Err(()))), Choice::Fallback));
        assert!(matches!(choice::<(), ()>(Err(())), Choice::Fallback));
    }
    #[test]
    fn tab_completes_files_and_directories() {
        let dir = std::env::temp_dir().join(format!("parvi-path-picker-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("folder")).unwrap();
        std::fs::write(dir.join("file one"), b"").unwrap();
        let prefix = format!("{}/fi", dir.display());
        assert_eq!(
            complete(&prefix),
            Some(format!("{}/file one", dir.display()))
        );
        assert_eq!(
            complete(&format!("{}/fo", dir.display())),
            Some(format!(
                "{}/folder{}",
                dir.display(),
                std::path::MAIN_SEPARATOR
            ))
        );
        assert_eq!(complete(&format!("{}/missing", dir.display())), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[gpui::test]
    fn path_field_completes_and_submits_a_file(cx: &mut gpui::TestAppContext) {
        let dir = std::env::temp_dir().join(format!("parvi-path-field-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("picked.txt");
        std::fs::write(&path, b"picked").unwrap();
        cx.update(|cx| {
            cx.set_global(crate::theme::Theme::violet());
            crate::widgets::field::bind_keys(cx);
        });
        let (tx, _rx) = async_channel::unbounded();
        let hub = cx.new(|_| crate::Hub::new(crate::link::socket::Net::start(), tx, None));
        let host = Host::new(&hub);
        let received = Rc::new(RefCell::new(Vec::new()));
        let selected = received.clone();
        let (picker, cx) = cx.add_window_view(move |window, cx| {
            Picker::new(
                host,
                "Attach",
                Box::new(move |paths, _| *selected.borrow_mut() = paths),
                window,
                cx,
            )
        });
        cx.update(|window, cx| {
            let focus = picker.read(cx).field.focus_handle(cx);
            window.focus(&focus, cx);
        });
        cx.simulate_input(&format!("{}/pick", dir.display()));
        cx.simulate_keystrokes("tab");
        assert_eq!(
            picker.read_with(cx, |p, cx| p.field.read(cx).text.clone()),
            path.to_string_lossy()
        );
        cx.simulate_keystrokes("enter");
        assert_eq!(*received.borrow(), vec![path]);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
