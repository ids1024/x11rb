// This is a (rough) port of hypnomoire from the xcb-demo repository.
// The original file has Copyright (C) 2001-2002 Bart Massey and Jamey Sharp and is licensed under
// a MIT license.

extern crate x11rb;

use std::f64::consts::PI;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use x11rb::connection::Connection;
use x11rb::errors::{ReplyError, ReplyOrIdError};
use x11rb::xproto::*;
use x11rb::{COPY_DEPTH_FROM_PARENT, Event};

/// Lag angle for the follow line
const LAG: f64 = 0.3;

/// Frames per second
const FRAME_RATE: u64 = 10;

/// Number of windows to show
const WINS: usize = 3;

#[derive(Default)]
struct Window {
    window: WINDOW,
    pixmap: PIXMAP,
    angle_velocity: f64,
}

fn main() {
    let (conn, screen_num) = x11rb::connect(None).unwrap();
    let conn = Arc::new(conn);
    let screen = &conn.setup().roots[screen_num];

    let white = conn.generate_id().unwrap();
    let black = conn.generate_id().unwrap();

    conn.create_gc(
        white,
        screen.root,
        &CreateGCAux::new()
            .graphics_exposures(0)
            .foreground(screen.white_pixel),
    )
    .unwrap();
    conn.create_gc(
        black,
        screen.root,
        &CreateGCAux::new()
            .graphics_exposures(0)
            .foreground(screen.black_pixel),
    )
    .unwrap();

    let windows: Vec<_> = (0..WINS)
        .map(|_| Arc::new(Mutex::new(Window::default())))
        .collect();

    for win in windows.iter() {
        let conn2 = Arc::clone(&conn);
        let win = Arc::clone(&win);
        thread::spawn(move || run(conn2, win, screen_num, white, black));
    }

    event_thread(conn, windows, white).unwrap();
}

fn run<C: Connection>(
    conn: Arc<C>,
    window_state: Arc<Mutex<Window>>,
    screen_num: usize,
    white: GCONTEXT,
    black: GCONTEXT,
) -> Result<(), ReplyOrIdError<C::Buf>> {
    let screen = &conn.setup().roots[screen_num];
    let default_size = 300;
    let pixmap = conn.generate_id()?;
    let window = conn.generate_id()?;

    {
        let mut guard = window_state.lock().unwrap();
        guard.pixmap = pixmap;
        guard.window = window;
        guard.angle_velocity = 0.05;
    }

    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        window,
        screen.root,
        0,            // x
        0,            // y
        default_size, // width
        default_size, // height
        0,
        WindowClass::InputOutput,
        screen.root_visual,
        &CreateWindowAux::new()
            .background_pixel(screen.white_pixel)
            .event_mask(EventMask::ButtonRelease | EventMask::Exposure | EventMask::StructureNotify)
            .do_not_propogate_mask(EventMask::ButtonPress),
    )?;
    conn.map_window(window)?;

    conn.create_pixmap(
        screen.root_depth,
        pixmap,
        window,
        default_size,
        default_size,
    )?;
    let rect = Rectangle {
        x: 0,
        y: 0,
        width: default_size,
        height: default_size,
    };
    conn.poly_fill_rectangle(pixmap, white, &[rect])?;

    let mut theta: f64 = 0.0;
    let radius = f64::from(default_size) + 1.0;

    loop {
        let guard = window_state.lock().unwrap();
        let (center_x, center_y) = (default_size as i16 / 2, default_size as i16 / 2);

        let (sin, cos) = theta.sin_cos();
        let (x, y) = ((radius * cos) as i16, (radius * sin) as i16);
        let lines = [
            Point {
                x: center_x,
                y: center_y,
            },
            Point { x, y },
        ];
        conn.poly_line(CoordMode::Previous, pixmap, black, &lines)?;

        let (sin, cos) = (theta + LAG).sin_cos();
        let (x, y) = ((radius * cos) as i16, (radius * sin) as i16);
        let lines = [
            Point {
                x: center_x,
                y: center_y,
            },
            Point { x, y },
        ];
        conn.poly_line(CoordMode::Previous, pixmap, white, &lines)?;

        conn.copy_area(
            pixmap,
            window,
            white,
            0,
            0,
            0,
            0,
            default_size,
            default_size,
        )?;
        conn.flush()?;

        theta += guard.angle_velocity;
        while theta > 2.0 * PI {
            theta -= 2.0 * PI;
        }
        while theta < 0.0 {
            theta += 2.0 * PI;
        }

        drop(guard);

        thread::sleep(Duration::from_micros(1_000_000 / FRAME_RATE));
    }
}

fn event_thread<C>(
    conn_arc: Arc<C>,
    windows: Vec<Arc<Mutex<Window>>>,
    white: GCONTEXT,
) -> Result<(), ReplyError<C::Buf>>
where
    C: Connection + Send + Sync + 'static,
{
    let mut first_window_mapped = false;

    let conn = &*conn_arc;

    loop {
        let event = conn.wait_for_event()?;
        let event = conn.parse_event(event)?;
        match event {
            Event::XprotoExposeEvent(event) => {
                if let Some(state) = find_window_by_id(&windows, event.window) {
                    let state = state.lock().unwrap();
                    conn.copy_area(
                        state.pixmap,
                        state.window,
                        white,
                        event.x as _,
                        event.y as _,
                        event.x as _,
                        event.y as _,
                        event.width,
                        event.height,
                    )?;
                    if event.count == 0 {
                        conn.flush()?;
                    }
                } else {
                    eprintln!("Expose on unknown window!");
                }
            }
            Event::XprotoButtonReleaseEvent(event) => {
                if let Some(state) = find_window_by_id(&windows, event.event) {
                    let mut state = state.lock().unwrap();
                    // FIXME: Make this matching somehow nicer
                    if event.detail == ButtonIndex::M1.into() {
                        state.angle_velocity *= -1.0;
                    } else if event.detail == ButtonIndex::M4.into() {
                        state.angle_velocity += 0.001;
                    } else if event.detail == ButtonIndex::M5.into() {
                        state.angle_velocity -= 0.001;
                    }
                } else {
                    eprintln!("ButtonRelease on unknown window!");
                }
            }
            Event::XprotoMapNotifyEvent(event) => {
                if !first_window_mapped {
                    first_window_mapped = true;
                    util::start_timeout_thread(conn_arc.clone(), event.window);
                }
            }
            Event::XprotoClientMessageEvent(_) => {
                // We simply assume that this is a message to close. Since we are the main thread,
                // everything closes when we exit.
                return Ok(());
            }
            Event::Error(err) => {
                eprintln!("Got an X11 error: {:?}", err);
            }
            _ => {}
        }
    }
}

fn find_window_by_id(
    windows: &[Arc<Mutex<Window>>],
    window: WINDOW,
) -> Option<&Arc<Mutex<Window>>> {
    windows.iter().find(|state| {
        state
            .lock()
            .map(|state| state.window == window)
            .unwrap_or(false)
    })
}

include!("integration_test_util/util.rs");
