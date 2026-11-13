//! New firmware projects: a blinking LED, ready to build, flash and debug, for the boards most
//! people start with.

use std::path::{Path, PathBuf};

pub struct Template {
    pub id: &'static str,
    pub name: &'static str,
    pub detail_en: &'static str,
    pub detail_pt: &'static str,
    /// The files, for a project of this name, and the one to open first.
    files: fn(&Names) -> Vec<(String, String)>,
    open: fn(&Names) -> String,
}

impl Template {
    pub fn detail(&self, pt: bool) -> &'static str {
        if pt { self.detail_pt } else { self.detail_en }
    }
}

/// A project's name in the forms each tool wants.
struct Names {
    /// As given, for comments and folders.
    title: String,
    /// Letters, digits and underscores, not starting with a digit: Arduino, CMake, Make.
    ident: String,
    /// Cargo's package name.
    krate: String,
}

impl Names {
    fn new(title: &str) -> Self {
        let mut ident: String = title.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
        if ident.is_empty() || ident.starts_with(|c: char| c.is_ascii_digit()) {
            ident.insert_str(0, "p_");
        }
        let krate = ident.to_ascii_lowercase().replace('_', "-").trim_matches('-').to_string();
        Self { title: title.to_string(), ident, krate: if krate.is_empty() { "firmware".into() } else { krate } }
    }
}

pub fn all() -> &'static [Template] {
    TEMPLATES
}

pub fn by_id(id: &str) -> Option<&'static Template> {
    TEMPLATES.iter().find(|t| t.id == id)
}

/// The folder an Arduino sketch must have: the same name as its .ino.
pub fn folder_name(template: &Template, name: &str) -> String {
    if template.id == "arduino" { Names::new(name).ident } else { name.trim().to_string() }
}

/// Writes a new project into `dir`, which must not exist or be empty. The file to open back.
pub fn create(template: &Template, dir: &Path, name: &str) -> Result<PathBuf, String> {
    let name = name.trim();
    if name.is_empty() || name.contains('/') {
        return Err("invalid project name".into());
    }
    if dir.exists() && std::fs::read_dir(dir).map_err(|e| e.to_string())?.next().is_some() {
        return Err(format!("{} already exists and isn't empty", dir.display()));
    }
    let names = Names::new(name);
    for (rel, text) in (template.files)(&names) {
        let p = dir.join(&rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    Ok(dir.join((template.open)(&names)))
}

const TEMPLATES: &[Template] = &[
    Template {
        id: "arduino",
        name: "Arduino (C++)",
        detail_en: "A sketch for arduino-cli: Uno, Nano, Mega, ESP32, Pico and every board it knows",
        detail_pt: "Um sketch para o arduino-cli: Uno, Nano, Mega, ESP32, Pico e todas as placas que ele conhece",
        files: arduino,
        open: |n| format!("{}.ino", n.ident),
    },
    Template {
        id: "platformio",
        name: "PlatformIO (C++)",
        detail_en: "Arduino framework on PlatformIO: Arduino Uno, ESP32 and STM32 Nucleo, one environment each",
        detail_pt: "Framework Arduino no PlatformIO: Arduino Uno, ESP32 e STM32 Nucleo, um ambiente para cada",
        files: platformio,
        open: |_| "src/main.cpp".into(),
    },
    Template {
        id: "pico-c",
        name: "Raspberry Pi Pico (C, Pico SDK)",
        detail_en: "Pico, Pico W, Pico 2: blinks the LED and writes over USB. Needs the Pico SDK",
        detail_pt: "Pico, Pico W, Pico 2: pisca o LED e escreve pela USB. Precisa do Pico SDK",
        files: pico_c,
        open: |_| "main.c".into(),
    },
    Template {
        id: "pico-rust",
        name: "Raspberry Pi Pico (Rust)",
        detail_en: "RP2040 without a HAL: cortex-m-rt and the registers. Flashes with elf2uf2-rs or probe-rs",
        detail_pt: "RP2040 sem HAL: cortex-m-rt e os registradores. Grava com o elf2uf2-rs ou o probe-rs",
        files: pico_rust,
        open: |_| "src/main.rs".into(),
    },
    Template {
        id: "stm32-c",
        name: "STM32F103 \"Blue Pill\" (C)",
        detail_en: "Bare metal, no HAL: Makefile, linker script and startup code. Flashes through an ST-Link",
        detail_pt: "Direto no hardware, sem HAL: Makefile, linker script e código de partida. Grava por um ST-Link",
        files: stm32_c,
        open: |_| "main.c".into(),
    },
    Template {
        id: "stm32-rust",
        name: "STM32F103 \"Blue Pill\" (Rust)",
        detail_en: "cortex-m-rt and the registers, no HAL. Flashes and debugs with probe-rs",
        detail_pt: "cortex-m-rt e os registradores, sem HAL. Grava e depura com o probe-rs",
        files: stm32_rust,
        open: |_| "src/main.rs".into(),
    },
];

fn arduino(n: &Names) -> Vec<(String, String)> {
    let ino = format!(
        r#"// {title}: pisca o LED da placa e escreve no monitor serial.
// Ctrl+F5 compila, grava e abre o monitor. Na primeira vez, o Tinta pergunta qual é a placa.

unsigned long contador = 0;

void setup() {{
  pinMode(LED_BUILTIN, OUTPUT);
  Serial.begin(9600);
}}

void loop() {{
  digitalWrite(LED_BUILTIN, HIGH);
  delay(250);
  digitalWrite(LED_BUILTIN, LOW);
  delay(250);
  Serial.print("Olá! ");
  Serial.println(contador++);
}}
"#,
        title = n.title
    );
    vec![(format!("{}.ino", n.ident), ino)]
}

fn platformio(n: &Names) -> Vec<(String, String)> {
    let ini = r#"; PlatformIO: https://docs.platformio.org/page/projectconf.html
; Ctrl+F5 builds, uploads and opens the monitor for the environment chosen in
; "Embedded: Choose Board…" (default_envs below, otherwise).

[platformio]
default_envs = uno

[env]
framework = arduino
monitor_speed = 115200

[env:uno]
platform = atmelavr
board = uno

[env:esp32]
platform = espressif32
board = esp32dev

[env:nucleo_f401re]
platform = ststm32
board = nucleo_f401re
debug_tool = stlink
"#;
    let main = format!(
        r#"// {title}: blinks the board's LED and writes to the serial monitor.
#include <Arduino.h>

#ifndef LED_BUILTIN
#define LED_BUILTIN 2
#endif

static unsigned long count = 0;

void setup() {{
  pinMode(LED_BUILTIN, OUTPUT);
  Serial.begin(115200);
}}

void loop() {{
  digitalWrite(LED_BUILTIN, HIGH);
  delay(250);
  digitalWrite(LED_BUILTIN, LOW);
  delay(250);
  Serial.print("Hello! ");
  Serial.println(count++);
}}
"#,
        title = n.title
    );
    vec![
        ("platformio.ini".into(), ini.into()),
        ("src/main.cpp".into(), main),
        (".gitignore".into(), ".pio/\ncompile_commands.json\n".into()),
    ]
}

fn pico_c(n: &Names) -> Vec<(String, String)> {
    let cmake = format!(
        r#"cmake_minimum_required(VERSION 3.13)

# pico, pico_w, pico2 or pico2_w: "Embedded: Choose Board…" passes it to CMake.
set(PICO_BOARD pico CACHE STRING "Board")

include(pico_sdk_import.cmake)
project({ident} C CXX ASM)
set(CMAKE_C_STANDARD 11)
pico_sdk_init()

add_executable({ident} main.c)
target_link_libraries({ident} pico_stdlib)
# The Pico W's LED hangs off the wireless chip.
if (PICO_CYW43_SUPPORTED)
    target_link_libraries({ident} pico_cyw43_arch_none)
endif ()

# printf goes out over USB, as a serial port.
pico_enable_stdio_usb({ident} 1)
pico_enable_stdio_uart({ident} 0)
# The .uf2 to copy to the board, beside the .elf.
pico_add_extra_outputs({ident})
"#,
        ident = n.ident
    );
    let import = r#"# Finds the Pico SDK: PICO_SDK_PATH, from the environment or given to CMake.
# Get it with: git clone --recurse-submodules https://github.com/raspberrypi/pico-sdk
if (DEFINED ENV{PICO_SDK_PATH} AND (NOT PICO_SDK_PATH))
    set(PICO_SDK_PATH $ENV{PICO_SDK_PATH})
endif ()
set(PICO_SDK_PATH "${PICO_SDK_PATH}" CACHE PATH "Path to the Raspberry Pi Pico SDK")
if (NOT PICO_SDK_PATH)
    message(FATAL_ERROR "Pico SDK not found: set PICO_SDK_PATH to its folder (git clone --recurse-submodules https://github.com/raspberrypi/pico-sdk)")
endif ()
get_filename_component(PICO_SDK_PATH "${PICO_SDK_PATH}" REALPATH BASE_DIR "${CMAKE_BINARY_DIR}")
if (NOT EXISTS ${PICO_SDK_PATH}/pico_sdk_init.cmake)
    message(FATAL_ERROR "${PICO_SDK_PATH} doesn't look like the Pico SDK")
endif ()
include(${PICO_SDK_PATH}/pico_sdk_init.cmake)
"#;
    let main = format!(
        r#"// {title}: blinks the LED and counts over USB. Ctrl+F5 builds, flashes and opens the
// serial monitor; F5 debugs, with a debug probe (a second Pico running debugprobe works).
#include <stdio.h>
#include "pico/stdlib.h"
#ifdef CYW43_WL_GPIO_LED_PIN
#include "pico/cyw43_arch.h"
#endif

static void led_init(void) {{
#if defined(PICO_DEFAULT_LED_PIN)
    gpio_init(PICO_DEFAULT_LED_PIN);
    gpio_set_dir(PICO_DEFAULT_LED_PIN, GPIO_OUT);
#elif defined(CYW43_WL_GPIO_LED_PIN)
    cyw43_arch_init();
#endif
}}

static void led_set(bool on) {{
#if defined(PICO_DEFAULT_LED_PIN)
    gpio_put(PICO_DEFAULT_LED_PIN, on);
#elif defined(CYW43_WL_GPIO_LED_PIN)
    cyw43_arch_gpio_put(CYW43_WL_GPIO_LED_PIN, on);
#endif
}}

int main(void) {{
    stdio_init_all();
    led_init();
    unsigned count = 0;
    while (true) {{
        led_set(true);
        sleep_ms(250);
        led_set(false);
        sleep_ms(250);
        printf("Hello! %u\n", count++);
    }}
}}
"#,
        title = n.title
    );
    vec![
        ("CMakeLists.txt".into(), cmake),
        ("pico_sdk_import.cmake".into(), import.into()),
        ("main.c".into(), main),
        (".gitignore".into(), "build/\n".into()),
    ]
}

/// What the Rust templates share: the build script that hands memory.x to the linker.
const BUILD_RS: &str = r#"//! Puts memory.x where the linker looks, also when this crate is in a workspace.
use std::{env, fs, path::PathBuf};

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::write(out.join("memory.x"), include_bytes!("memory.x")).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory.x");
}
"#;

fn cargo_toml(n: &Names, extra: &str) -> String {
    format!(
        r#"[package]
name = "{krate}"
version = "0.1.0"
edition = "2021"

[dependencies]
cortex-m = "0.7"
cortex-m-rt = "0.7"
panic-halt = "0.2"
{extra}
# Small enough to fit, and still debuggable.
[profile.dev]
opt-level = "s"

[profile.release]
debug = 2
"#,
        krate = n.krate
    )
}

fn pico_rust(n: &Names) -> Vec<(String, String)> {
    let config = r#"[build]
target = "thumbv6m-none-eabi" # Cortex-M0+: the RP2040

[target.thumbv6m-none-eabi]
# Over USB: hold BOOTSEL while plugging the Pico in, then Ctrl+F5 (cargo install elf2uf2-rs).
# With a debug probe instead: runner = "probe-rs run --chip RP2040"
runner = "elf2uf2-rs -d"
rustflags = ["-C", "link-arg=--nmagic", "-C", "link-arg=-Tlink.x"]
"#;
    let memory = r#"MEMORY {
    BOOT2 : ORIGIN = 0x10000000, LENGTH = 0x100
    FLASH : ORIGIN = 0x10000100, LENGTH = 2048K - 0x100
    RAM   : ORIGIN = 0x20000000, LENGTH = 256K
}

EXTERN(BOOT2_FIRMWARE)

SECTIONS {
    /* The second-stage bootloader the RP2040's ROM runs first, from the first 256 bytes. */
    .boot2 ORIGIN(BOOT2) :
    {
        KEEP(*(.boot2));
    } > BOOT2
} INSERT BEFORE .text;
"#;
    let main = format!(
        r#"//! {title}: blinks the Pico's LED (GPIO 25), straight through the registers.
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use panic_halt as _;

/// Sets the flash up for the program to run from it.
#[link_section = ".boot2"]
#[used]
pub static BOOT2_FIRMWARE: [u8; 256] = rp2040_boot2::BOOT_LOADER_W25Q080;

const RESETS: usize = 0x4000_c000;
/// Writing here clears bits of RESETS, atomically.
const RESETS_CLEAR: *mut u32 = (RESETS + 0x3000) as *mut u32;
const RESETS_DONE: *const u32 = (RESETS + 0x8) as *const u32;
const IO_BANK0: u32 = 1 << 5;
const PADS_BANK0: u32 = 1 << 8;
const GPIO25_CTRL: *mut u32 = (0x4001_4000 + 0x0cc) as *mut u32;
const SIO_GPIO_OE_SET: *mut u32 = 0xd000_0024 as *mut u32;
const SIO_GPIO_OUT_XOR: *mut u32 = 0xd000_001c as *mut u32;
const LED: u32 = 1 << 25;

#[entry]
fn main() -> ! {{
    unsafe {{
        // The GPIO blocks start in reset.
        RESETS_CLEAR.write_volatile(IO_BANK0 | PADS_BANK0);
        while RESETS_DONE.read_volatile() & (IO_BANK0 | PADS_BANK0) != (IO_BANK0 | PADS_BANK0) {{}}
        // GPIO 25 driven by the processor (function 5, SIO), as an output.
        GPIO25_CTRL.write_volatile(5);
        SIO_GPIO_OE_SET.write_volatile(LED);
    }}
    loop {{
        unsafe {{ SIO_GPIO_OUT_XOR.write_volatile(LED) }};
        // About a quarter of a second on the ring oscillator the chip starts with.
        cortex_m::asm::delay(1_500_000);
    }}
}}
"#,
        title = n.title
    );
    vec![
        ("Cargo.toml".into(), cargo_toml(n, "rp2040-boot2 = \"0.3\"\n")),
        (".cargo/config.toml".into(), config.into()),
        // probe-rs and cargo-embed read the chip from here; so does the editor, for F5.
        ("Embed.toml".into(), "[default.general]\nchip = \"RP2040\"\n".into()),
        ("memory.x".into(), memory.into()),
        ("build.rs".into(), BUILD_RS.into()),
        ("src/main.rs".into(), main),
        (".gitignore".into(), "target/\n".into()),
    ]
}

fn stm32_rust(n: &Names) -> Vec<(String, String)> {
    let config = r#"[build]
target = "thumbv7m-none-eabi" # Cortex-M3: the STM32F103

[target.thumbv7m-none-eabi]
# cargo run flashes through the probe (an ST-Link) and stays to show the board's log.
runner = "probe-rs run --chip STM32F103C8"
rustflags = ["-C", "link-arg=-Tlink.x"]
"#;
    let memory = "MEMORY {\n    FLASH : ORIGIN = 0x08000000, LENGTH = 64K\n    RAM   : ORIGIN = 0x20000000, LENGTH = 20K\n}\n";
    let main = format!(
        r#"//! {title}: blinks the Blue Pill's LED (PC13), straight through the registers.
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use panic_halt as _;

const RCC_APB2ENR: *mut u32 = 0x4002_1018 as *mut u32;
const IOPCEN: u32 = 1 << 4;
const GPIOC_CRH: *mut u32 = 0x4001_1004 as *mut u32;
const GPIOC_ODR: *mut u32 = 0x4001_100c as *mut u32;
const LED: u32 = 1 << 13;

#[entry]
fn main() -> ! {{
    unsafe {{
        // The clock to port C, then PC13 as a push-pull output at 2 MHz (mode 0b10, cnf 0b00).
        RCC_APB2ENR.write_volatile(RCC_APB2ENR.read_volatile() | IOPCEN);
        let crh = GPIOC_CRH.read_volatile() & !(0xf << 20);
        GPIOC_CRH.write_volatile(crh | (0b0010 << 20));
    }}
    loop {{
        unsafe {{ GPIOC_ODR.write_volatile(GPIOC_ODR.read_volatile() ^ LED) }};
        // About a quarter of a second on the 8 MHz internal oscillator.
        cortex_m::asm::delay(2_000_000);
    }}
}}
"#,
        title = n.title
    );
    vec![
        ("Cargo.toml".into(), cargo_toml(n, "")),
        (".cargo/config.toml".into(), config.into()),
        ("memory.x".into(), memory.into()),
        ("build.rs".into(), BUILD_RS.into()),
        ("src/main.rs".into(), main),
        (".gitignore".into(), "target/\n".into()),
    ]
}

/// The STM32F103's memory, for the linker. Public for the tests that run it in an emulator,
/// whose chip has less RAM.
pub const STM32F103_LD: &str = r#"/* STM32F103C8: 64 KiB of flash, 20 KiB of RAM. */
MEMORY
{
  FLASH (rx)  : ORIGIN = 0x08000000, LENGTH = 64K
  RAM   (rwx) : ORIGIN = 0x20000000, LENGTH = 20K
}

/* The stack starts at the top of RAM and grows down. */
_estack = ORIGIN(RAM) + LENGTH(RAM);

ENTRY(Reset_Handler)

SECTIONS
{
  .isr_vector :
  {
    KEEP(*(.isr_vector))
  } > FLASH

  .text :
  {
    *(.text*)
    *(.rodata*)
    . = ALIGN(4);
  } > FLASH

  .ARM.exidx :
  {
    *(.ARM.exidx*)
  } > FLASH

  /* Initial values of variables: kept in flash, copied to RAM at start. */
  _sidata = LOADADDR(.data);
  .data :
  {
    _sdata = .;
    *(.data*)
    . = ALIGN(4);
    _edata = .;
  } > RAM AT > FLASH

  .bss (NOLOAD) :
  {
    _sbss = .;
    *(.bss*)
    *(COMMON)
    . = ALIGN(4);
    _ebss = .;
  } > RAM
}
"#;

pub const STM32F103_STARTUP: &str = r#"// What runs before main: the vector table, and the RAM set up as C expects it.
#include <stdint.h>

extern uint32_t _sidata, _sdata, _edata, _sbss, _ebss, _estack;
int main(void);

void Reset_Handler(void) {
    // Initialized variables get their values; the others start at zero.
    uint32_t *src = &_sidata;
    for (uint32_t *dst = &_sdata; dst < &_edata;) {
        *dst++ = *src++;
    }
    for (uint32_t *dst = &_sbss; dst < &_ebss;) {
        *dst++ = 0;
    }
    main();
    for (;;) {
    }
}

// A fault stops here: look at the call stack in the debugger.
void Default_Handler(void) {
    for (;;) {
    }
}

void NMI_Handler(void) __attribute__((weak, alias("Default_Handler")));
void HardFault_Handler(void) __attribute__((weak, alias("Default_Handler")));
void MemManage_Handler(void) __attribute__((weak, alias("Default_Handler")));
void BusFault_Handler(void) __attribute__((weak, alias("Default_Handler")));
void UsageFault_Handler(void) __attribute__((weak, alias("Default_Handler")));
void SVC_Handler(void) __attribute__((weak, alias("Default_Handler")));
void PendSV_Handler(void) __attribute__((weak, alias("Default_Handler")));
void SysTick_Handler(void) __attribute__((weak, alias("Default_Handler")));

__attribute__((section(".isr_vector"), used)) void (*const vectors[16])(void) = {
    (void (*)(void))(&_estack),
    Reset_Handler,
    NMI_Handler,
    HardFault_Handler,
    MemManage_Handler,
    BusFault_Handler,
    UsageFault_Handler,
    0,
    0,
    0,
    0,
    SVC_Handler,
    0,
    0,
    PendSV_Handler,
    SysTick_Handler,
};
"#;

fn stm32_c(n: &Names) -> Vec<(String, String)> {
    let make = format!(
        r#"# STM32F103C8 ("Blue Pill"). make builds; make flash puts it on the board through an ST-Link.
TARGET = {ident}

CC = arm-none-eabi-gcc
OBJCOPY = arm-none-eabi-objcopy
SIZE = arm-none-eabi-size

# -O0: every variable stays visible in the debugger.
CFLAGS = -mcpu=cortex-m3 -mthumb -std=c11 -O0 -g3 -Wall -Wextra -ffunction-sections -fdata-sections
LDFLAGS = -T stm32f103c8.ld -nostartfiles --specs=nano.specs --specs=nosys.specs -Wl,--gc-sections -Wl,-Map=build/$(TARGET).map

SRCS = startup.c main.c
OBJS = $(SRCS:%.c=build/%.o)

all: build/$(TARGET).elf build/$(TARGET).bin

build:
	mkdir -p build

build/%.o: %.c | build
	$(CC) $(CFLAGS) -c $< -o $@

build/$(TARGET).elf: $(OBJS) stm32f103c8.ld
	$(CC) $(CFLAGS) $(OBJS) $(LDFLAGS) -o $@
	$(SIZE) $@

build/$(TARGET).bin: build/$(TARGET).elf
	$(OBJCOPY) -O binary $< $@

flash: build/$(TARGET).elf
	openocd -f interface/stlink.cfg -f target/stm32f1x.cfg -c "program $< verify reset exit"

clean:
	rm -rf build

.PHONY: all flash clean
"#,
        ident = n.ident
    );
    let main = format!(
        r#"// {title}: blinks the Blue Pill's LED (PC13), straight through the registers.
// Ctrl+F5 builds and flashes; F5 debugs, stopping at main.
#include <stdint.h>

#define REG(addr) (*(volatile uint32_t *)(addr))
#define RCC_APB2ENR REG(0x40021018)
#define GPIOC_CRH REG(0x40011004)
#define GPIOC_ODR REG(0x4001100C)
#define IOPCEN (1u << 4)
#define LED (1u << 13)

static void wait(uint32_t n) {{
    for (volatile uint32_t i = 0; i < n; i++) {{
    }}
}}

int main(void) {{
    // The clock to port C, then PC13 as a push-pull output at 2 MHz (mode 0b10, cnf 0b00).
    RCC_APB2ENR |= IOPCEN;
    GPIOC_CRH = (GPIOC_CRH & ~(0xFu << 20)) | (0x2u << 20);
    uint32_t blinks = 0;
    for (;;) {{
        GPIOC_ODR ^= LED;
        blinks++;
        wait(200000);
    }}
}}
"#,
        title = n.title
    );
    let flags = "--target=arm-none-eabi\n-mcpu=cortex-m3\n-mthumb\n-std=c11\n-ffreestanding\n";
    vec![
        ("Makefile".into(), make),
        ("stm32f103c8.ld".into(), STM32F103_LD.into()),
        ("startup.c".into(), STM32F103_STARTUP.into()),
        ("main.c".into(), main),
        // For clangd, which has no compile_commands.json from a plain Makefile.
        ("compile_flags.txt".into(), flags.into()),
        (".tinta/embedded.json".into(), "// The chip, for flashing and debugging through a probe.\n{\n  \"chip\": \"STM32F103C8\"\n}\n".into()),
        (".gitignore".into(), "build/\n".into()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedded::{Kind, detect_with, prefs::Prefs};

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tinta-template-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn every_template_makes_a_project_the_editor_recognizes() {
        for (t, kind) in TEMPLATES.iter().zip([Kind::Arduino, Kind::PlatformIO, Kind::PicoSdk, Kind::Cargo, Kind::Firmware, Kind::Cargo]) {
            let name = "Meu Pisca-pisca";
            let d = dir(t.id).join(folder_name(t, name));
            let open = create(t, &d, name).unwrap();
            assert!(open.is_file(), "{}: {}", t.id, open.display());
            let config = crate::embedded::load_config(&d).unwrap();
            let p = detect_with(&d, config, &Prefs::default()).unwrap_or_else(|| panic!("{} isn't recognized", t.id));
            assert_eq!(p.kind, kind, "{}", t.id);
            assert!(create(t, &d, name).is_err(), "never over an existing project");
        }
        let d = dir("names").join(folder_name(by_id("arduino").unwrap(), "2 leds"));
        assert!(d.ends_with("p_2_leds"));
        let open = create(by_id("arduino").unwrap(), &d, "2 leds").unwrap();
        assert!(open.ends_with("p_2_leds/p_2_leds.ino"));
        let rust = dir("krate");
        create(by_id("stm32-rust").unwrap(), &rust, "Meu Pisca").unwrap();
        assert!(std::fs::read_to_string(rust.join("Cargo.toml")).unwrap().contains("name = \"meu-pisca\""));
        let p = detect_with(&rust, None, &Prefs::default()).unwrap();
        assert_eq!(p.chip.as_deref(), Some("STM32F103C8"));
        assert_eq!(p.target.as_deref(), Some("thumbv7m-none-eabi"));
        let pico = dir("pico-rust");
        create(by_id("pico-rust").unwrap(), &pico, "pisca").unwrap();
        assert_eq!(detect_with(&pico, None, &Prefs::default()).unwrap().chip.as_deref(), Some("RP2040"));
    }

    /// The C template builds with the ARM toolchain, when it's installed.
    #[test]
    fn the_stm32_c_template_builds() {
        if !crate::host::find("arm-none-eabi-gcc") || !crate::host::find("make") {
            eprintln!("arm-none-eabi-gcc not installed, skipping");
            return;
        }
        let d = dir("stm32-c-build");
        create(by_id("stm32-c").unwrap(), &d, "pisca").unwrap();
        let out = std::process::Command::new("make").current_dir(&d).output().unwrap();
        assert!(out.status.success(), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        let elf = d.join("build/pisca.elf");
        assert!(crate::embedded::is_elf(&elf));
        let p = detect_with(&d, crate::embedded::load_config(&d).unwrap(), &Prefs::default()).unwrap();
        assert_eq!(p.firmware().as_deref(), Some(elf.as_path()));
    }
}
