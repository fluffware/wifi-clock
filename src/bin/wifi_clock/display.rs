use core::future::Future;
use defmt::debug;
use embassy_rp::gpio::{AnyPin, Level, Output};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::block_for;
use embassy_time::{Duration, Instant, Timer};
use static_cell::StaticCell;
use core::ops::Range;
/*

+-A-+
F   B
+-G-+
E   C
+-D-+

*/
const SEG_A: u8 = 0x80;
const SEG_B: u8 = 0x40;
const SEG_C: u8 = 0x02;
const SEG_D: u8 = 0x04;
const SEG_E: u8 = 0x01;
const SEG_F: u8 = 0x10;
const SEG_G: u8 = 0x20;

// Colon
const SEG_UPPER: u8 = 0x20;
const SEG_LOWER: u8 = 0x02;

const DIGIT_POSITIONS: [u8; 5] = [0x40, 0x80, 0x10, 0x01, 0x20];
const SYMBOLS: [u8; 20] = [
    SEG_A | SEG_B | SEG_C | SEG_D | SEG_E | SEG_F, // 0
    SEG_B | SEG_C,                                 // 1
    SEG_A | SEG_B | SEG_G | SEG_E | SEG_D,         // 2
    SEG_A | SEG_B | SEG_G | SEG_C | SEG_D,         // 3
    SEG_F | SEG_B | SEG_G | SEG_C,                 // 4
    SEG_A | SEG_F | SEG_G | SEG_C | SEG_D,         // 5
    SEG_A | SEG_F | SEG_E | SEG_D | SEG_C | SEG_G, // 6
    SEG_A | SEG_B | SEG_C,                         // 7
    SEG_A | SEG_B | SEG_C | SEG_D | SEG_E | SEG_F | SEG_G, // 8
    SEG_G | SEG_F | SEG_A | SEG_B | SEG_C | SEG_D, //9
    SEG_A | SEG_F | SEG_B | SEG_G | SEG_E | SEG_C, // A
    SEG_F | SEG_G | SEG_E | SEG_C | SEG_D,         //b
    SEG_A | SEG_F | SEG_E | SEG_D,                 // C
    SEG_B | SEG_E | SEG_D | SEG_C | SEG_G,         // d
    SEG_A | SEG_F | SEG_G | SEG_E | SEG_D,         // E
    SEG_A | SEG_F | SEG_G | SEG_E,                 // F
    0,                                             // Blank
    SEG_UPPER | SEG_LOWER,                         // Colon
    SEG_UPPER,                                     // Colon upper
    SEG_LOWER,                                     // Colon lower
];

pub const SYM_BLANK: u8 = 16;
pub const SYM_COLON: u8 = SYM_BLANK + 1;
pub const SYM_COLON_UPPER: u8 = SYM_COLON + 1;
pub const SYM_COLON_LOWER: u8 = SYM_COLON_UPPER + 1;

pub struct LedBus<'a> {
    data: [Output<'a, AnyPin>; 8],
    enable: Output<'a, AnyPin>,
    addr_sel: Output<'a, AnyPin>,
    write: Output<'a, AnyPin>, // Low write, high read
}

impl<'a> LedBus<'a> {
    pub fn new(
        data: [Output<'a, AnyPin>; 8],
        enable: Output<'a, AnyPin>,
        addr_sel: Output<'a, AnyPin>,
        write: Output<'a, AnyPin>,
    ) -> Self {
        Self {
            data,
            enable,
            addr_sel,
            write,
        }
    }
    fn set_data_pins(&mut self, mut data: u8) {
        for i in 0..8 {
            let level = Level::from(data & 1 != 0);
            self.data[i].set_level(level);
            data >>= 1;
        }
    }
    pub fn write_data(&mut self, addr: u8, data: u8) {
        self.enable.set_level(Level::Low);
        self.write.set_level(Level::Low);
        self.addr_sel.set_level(Level::Low);
        self.set_data_pins(addr);
        self.addr_sel.set_level(Level::High);
        block_for(Duration::from_micros(2));
        self.addr_sel.set_level(Level::Low);
        self.set_data_pins(data);
        self.enable.set_level(Level::High);
        block_for(Duration::from_micros(2));
        self.enable.set_level(Level::Low);
    }
}

#[derive(Clone)]
pub struct DisplayControl {
    disp: &'static Mutex<ThreadModeRawMutex, Display>,
}

impl DisplayControl {
    pub async fn set_sym(&self, pos: usize, sym: u8) {
        (*self.disp.lock().await).syms[pos] = sym;
    }
    
    pub async fn set_sym_range(&self,  range: Range<usize>, sym: u8) {
	let mut disp = self.disp.lock().await;
	for d in range {
	    disp.syms[d] = sym;
	}
    }

    pub async fn set_int(&self, mut range: Range<usize>, mut val: u16) {
	let mut disp = self.disp.lock().await;
	while let Some(d) = range.next_back()  {
	    let sym = (val % 10) as u8;
	    disp.syms[d] = sym;
	    val /= 10;
	}
    }
}
struct Display {
    syms: [u8; 5],
}

pub fn new(led_bus: LedBus<'static>) -> (DisplayControl, impl Future<Output = bool>) {
    static STATIC_CELL: StaticCell<Mutex<ThreadModeRawMutex, Display>> = StaticCell::new();
    let disp = STATIC_CELL.init_with(move || {
        Mutex::new(Display {
            syms: [SYM_BLANK; 5],
        })
    });

    (DisplayControl { disp }, display_runner(led_bus, disp))
}

async fn display_runner(
    mut led_bus: LedBus<'static>,
    disp: &'static Mutex<ThreadModeRawMutex, Display>,
) -> bool {
    loop {
        let syms = (*disp.lock().await).syms.clone();
        for d in 0..DIGIT_POSITIONS.len() {
            led_bus.write_data(1, 0);
            led_bus.write_data(0, DIGIT_POSITIONS[d]);
            led_bus.write_data(1, SYMBOLS[syms[d] as usize]);
            Timer::after(Duration::from_millis(2)).await;
        }
    }
}
