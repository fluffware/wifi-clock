use super::display::{
    self, DisplayControl, SYM_BLANK, SYM_COLON, SYM_COLON_LOWER, SYM_COLON_UPPER,
};
use core::future::Future;
use defmt::debug;
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::{Duration, Instant, Ticker, Timer};
use futures::StreamExt;
use static_cell::StaticCell;

pub struct ClockControl {
    clock: &'static Mutex<ThreadModeRawMutex, Clock>,
}

impl ClockControl
{
    pub async fn stopwatch_start(&self)
    {
	debug!("Start");
	self.clock.lock().await.state = ClockState::StopWatchRunning;
	debug!("Start done");
    }

    pub async fn stopwatch_stop(&self)
    {
	debug!("Stop");
	self.clock.lock().await.state = ClockState::StopWatchStopped;
    }

    pub async fn stopwatch_reset(&self)
    {
	let mut clock = self.clock.lock().await;
	clock.state = ClockState::StopWatchStopped;
	clock.run_time = 0;
    }
    
}

#[derive(Copy, Clone)]
enum ClockState {
    Startup,
    Time,
    StopWatchStopped,
    StopWatchRunning,
}

struct Clock {
    state: ClockState,
    run_time: u32,
}

pub fn new(disp: DisplayControl) -> (ClockControl, impl Future<Output = bool>) {
    static STATIC_CELL: StaticCell<Mutex<ThreadModeRawMutex, Clock>> = StaticCell::new();
    let clock = STATIC_CELL.init_with(move || {
        Mutex::new(Clock {
            state: ClockState::Startup,
            run_time: 0,
        })
    });

    (ClockControl { clock }, clock_runner(disp, clock))
}

async fn clock_runner(
    disp: DisplayControl,
    clock: &'static Mutex<ThreadModeRawMutex, Clock>,
) -> bool {
    let mut ticker = Ticker::every(Duration::from_millis(500));
    loop {
	let state = {
	    let clock = clock.lock().await;
	    clock.state
	};
	match state {
	    ClockState::Startup => {
		disp.set_sym_range(0..4, 0).await;
		disp.set_sym(4, display::SYM_COLON).await;
		ticker.next().await;
		disp.set_sym_range(0..5, display::SYM_BLANK).await;
	    }
	    ClockState::StopWatchStopped | ClockState::StopWatchRunning => {
		let mut clock = clock.lock().await;
		let mut run_time = clock.run_time;
		if let ClockState::StopWatchRunning = state {
		    run_time += 1;
		    clock.run_time = run_time;
		}
		disp.set_sym(4, display::SYM_COLON).await;
		disp.set_int(2..4, ((run_time / 2) % 60) as u16).await;
		disp.set_int(0..2, (run_time / (2*  60)) as u16).await;
	    }
	    _ => {}
	}
	ticker.next().await;	    
    }
}
