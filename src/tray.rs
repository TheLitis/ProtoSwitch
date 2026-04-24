use std::thread;

use anyhow::Context;
use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder, TrayIconEvent};

use crate::app;
use crate::paths::AppPaths;
use crate::{APP_NAME, APP_VERSION};

#[derive(Debug)]
enum UserEvent {
    Tray,
    Menu(MenuEvent),
}

pub fn run(paths: &AppPaths) -> anyhow::Result<()> {
    match app::ensure_watcher_running(paths) {
        Ok(true) => {
            let _ = paths.append_log("tray started watcher");
        }
        Ok(false) => {
            let _ = paths.append_log("tray found watcher already running");
        }
        Err(error) => {
            let _ = paths.append_log(format!("tray could not start watcher: {error:#}"));
        }
    }

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    TrayIconEvent::set_event_handler(Some(move |event| {
        let _ = event;
        let _ = proxy.send_event(UserEvent::Tray);
    }));
    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |event| {
        let _ = proxy.send_event(UserEvent::Menu(event));
    }));

    let menu = Menu::new();
    let switch_item = MenuItem::new("Найти новый proxy", true, None);
    let restart_item = MenuItem::new("Перезапустить watcher", true, None);
    let stop_item = MenuItem::new("Остановить ProtoSwitch", true, None);
    let quit_item = MenuItem::new("Скрыть индикатор", true, None);
    menu.append_items(&[
        &switch_item,
        &restart_item,
        &PredefinedMenuItem::separator(),
        &stop_item,
        &quit_item,
    ])
    .context("Не удалось собрать tray menu")?;

    let paths_for_events = paths.clone();
    let icon = load_icon()?;
    let mut tray_icon = None;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::NewEvents(StartCause::Init) => {
                tray_icon = Some(
                    TrayIconBuilder::new()
                        .with_menu(Box::new(menu.clone()))
                        .with_tooltip(format!("{APP_NAME} {APP_VERSION}"))
                        .with_icon(icon.clone())
                        .build()
                        .expect("Не удалось создать tray icon"),
                );
            }
            Event::UserEvent(UserEvent::Menu(event)) => {
                let paths = paths_for_events.clone();
                if event.id == switch_item.id() {
                    thread::spawn(move || {
                        match app::switch_to_candidate(&paths, false) {
                            Ok(message) => {
                                let _ = paths.append_log(format!("tray switch: {message}"));
                            }
                            Err(error) => {
                                let _ = paths.append_log(format!("tray switch failed: {error:#}"));
                            }
                        }
                    });
                } else if event.id == restart_item.id() {
                    thread::spawn(move || {
                        match app::restart_background_watcher(&paths) {
                            Ok(message) => {
                                let _ = paths.append_log(format!("tray restart: {message}"));
                            }
                            Err(error) => {
                                let _ = paths.append_log(format!("tray restart failed: {error:#}"));
                            }
                        }
                    });
                } else if event.id == stop_item.id() {
                    match app::stop_all_protoswitch_processes(&paths) {
                        Ok(stopped) => {
                            let _ = paths.append_log(format!("tray stopped ProtoSwitch: {stopped}"));
                        }
                        Err(error) => {
                            let _ = paths.append_log(format!("tray stop failed: {error:#}"));
                        }
                    }
                    tray_icon.take();
                    *control_flow = ControlFlow::Exit;
                } else if event.id == quit_item.id() {
                    let _ = paths.append_log("tray indicator hidden");
                    tray_icon.take();
                    *control_flow = ControlFlow::Exit;
                }
            }
            Event::UserEvent(UserEvent::Tray) => {}
            _ => {}
        }
    });
}

fn load_icon() -> anyhow::Result<Icon> {
    let image = image::load_from_memory(include_bytes!("../assets/windows/protoswitch.png"))
        .context("Не удалось загрузить tray icon")?
        .into_rgba8();
    let (width, height) = image.dimensions();
    Icon::from_rgba(image.into_raw(), width, height).context("Не удалось подготовить tray icon")
}
