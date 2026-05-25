//! Windows virtual-key constants exposed as a Lua global table.
//!
//! UE4SS scripts conventionally write `RegisterKeyBind(Key.F5, fn)`. To make
//! community scripts portable, we preload the same `Key` table with the
//! standard `VK_*` constants from `windows::Win32::UI::Input::KeyboardAndMouse`.
//!
//! Only the common keys are populated: function keys, letters, digits,
//! numpad, arrows, plus the modifier + nav cluster. That's enough for
//! virtually every shipped UE4SS keybind script; if a niche script needs
//! e.g. `VK_OEM_3`, the integer literal works fine — Lua doesn't care
//! whether the value came from the `Key` table or not.

use mlua::Lua;

#[cfg(windows)]
use windows::Win32::UI::Input::KeyboardAndMouse as vk;

/// Static `(name, vk)` table. On non-Windows builds the keys collapse to
/// raw u32s borrowed from MSDN so the table layout stays identical and
/// scripts authored on Windows can still parse-test on a Linux CI runner.
fn key_table() -> Vec<(&'static str, u32)> {
    #[cfg(windows)]
    {
        vec![
            // Function keys
            ("F1", vk::VK_F1.0 as u32),
            ("F2", vk::VK_F2.0 as u32),
            ("F3", vk::VK_F3.0 as u32),
            ("F4", vk::VK_F4.0 as u32),
            ("F5", vk::VK_F5.0 as u32),
            ("F6", vk::VK_F6.0 as u32),
            ("F7", vk::VK_F7.0 as u32),
            ("F8", vk::VK_F8.0 as u32),
            ("F9", vk::VK_F9.0 as u32),
            ("F10", vk::VK_F10.0 as u32),
            ("F11", vk::VK_F11.0 as u32),
            ("F12", vk::VK_F12.0 as u32),
            ("F13", vk::VK_F13.0 as u32),
            ("F14", vk::VK_F14.0 as u32),
            ("F15", vk::VK_F15.0 as u32),
            ("F16", vk::VK_F16.0 as u32),
            ("F17", vk::VK_F17.0 as u32),
            ("F18", vk::VK_F18.0 as u32),
            ("F19", vk::VK_F19.0 as u32),
            ("F20", vk::VK_F20.0 as u32),
            ("F21", vk::VK_F21.0 as u32),
            ("F22", vk::VK_F22.0 as u32),
            ("F23", vk::VK_F23.0 as u32),
            ("F24", vk::VK_F24.0 as u32),
            // Digits 0..9 — letters + digits use the ASCII codepoints,
            // which is exactly what the Win32 docs specify for VK_0..VK_9
            // and VK_A..VK_Z (no dedicated constants in `windows-rs`).
            ("ZERO", 0x30),
            ("ONE", 0x31),
            ("TWO", 0x32),
            ("THREE", 0x33),
            ("FOUR", 0x34),
            ("FIVE", 0x35),
            ("SIX", 0x36),
            ("SEVEN", 0x37),
            ("EIGHT", 0x38),
            ("NINE", 0x39),
            // Letters A..Z
            ("A", 0x41),
            ("B", 0x42),
            ("C", 0x43),
            ("D", 0x44),
            ("E", 0x45),
            ("F", 0x46),
            ("G", 0x47),
            ("H", 0x48),
            ("I", 0x49),
            ("J", 0x4A),
            ("K", 0x4B),
            ("L", 0x4C),
            ("M", 0x4D),
            ("N", 0x4E),
            ("O", 0x4F),
            ("P", 0x50),
            ("Q", 0x51),
            ("R", 0x52),
            ("S", 0x53),
            ("T", 0x54),
            ("U", 0x55),
            ("V", 0x56),
            ("W", 0x57),
            ("X", 0x58),
            ("Y", 0x59),
            ("Z", 0x5A),
            // Numpad
            ("NUMPAD0", vk::VK_NUMPAD0.0 as u32),
            ("NUMPAD1", vk::VK_NUMPAD1.0 as u32),
            ("NUMPAD2", vk::VK_NUMPAD2.0 as u32),
            ("NUMPAD3", vk::VK_NUMPAD3.0 as u32),
            ("NUMPAD4", vk::VK_NUMPAD4.0 as u32),
            ("NUMPAD5", vk::VK_NUMPAD5.0 as u32),
            ("NUMPAD6", vk::VK_NUMPAD6.0 as u32),
            ("NUMPAD7", vk::VK_NUMPAD7.0 as u32),
            ("NUMPAD8", vk::VK_NUMPAD8.0 as u32),
            ("NUMPAD9", vk::VK_NUMPAD9.0 as u32),
            // Whitespace / control
            ("SPACE", vk::VK_SPACE.0 as u32),
            ("TAB", vk::VK_TAB.0 as u32),
            ("ENTER", vk::VK_RETURN.0 as u32),
            ("RETURN", vk::VK_RETURN.0 as u32),
            ("ESCAPE", vk::VK_ESCAPE.0 as u32),
            ("BACKSPACE", vk::VK_BACK.0 as u32),
            // Arrows
            ("UP", vk::VK_UP.0 as u32),
            ("DOWN", vk::VK_DOWN.0 as u32),
            ("LEFT", vk::VK_LEFT.0 as u32),
            ("RIGHT", vk::VK_RIGHT.0 as u32),
            // Modifiers
            ("LEFT_SHIFT", vk::VK_LSHIFT.0 as u32),
            ("RIGHT_SHIFT", vk::VK_RSHIFT.0 as u32),
            ("LEFT_CONTROL", vk::VK_LCONTROL.0 as u32),
            ("RIGHT_CONTROL", vk::VK_RCONTROL.0 as u32),
            ("LEFT_ALT", vk::VK_LMENU.0 as u32),
            ("RIGHT_ALT", vk::VK_RMENU.0 as u32),
            // Nav cluster
            ("PAGE_UP", vk::VK_PRIOR.0 as u32),
            ("PAGE_DOWN", vk::VK_NEXT.0 as u32),
            ("HOME", vk::VK_HOME.0 as u32),
            ("END", vk::VK_END.0 as u32),
            ("INSERT", vk::VK_INSERT.0 as u32),
            ("DELETE", vk::VK_DELETE.0 as u32),
        ]
    }
    #[cfg(not(windows))]
    {
        // Mirror the Win32 codes so the Lua API matches across platforms.
        // Non-Windows builds exist only for CI; the keybind poll thread is
        // a no-op there.
        vec![
            ("F1", 0x70),
            ("F2", 0x71),
            ("F3", 0x72),
            ("F4", 0x73),
            ("F5", 0x74),
            ("F6", 0x75),
            ("F7", 0x76),
            ("F8", 0x77),
            ("F9", 0x78),
            ("F10", 0x79),
            ("F11", 0x7A),
            ("F12", 0x7B),
            ("F13", 0x7C),
            ("F14", 0x7D),
            ("F15", 0x7E),
            ("F16", 0x7F),
            ("F17", 0x80),
            ("F18", 0x81),
            ("F19", 0x82),
            ("F20", 0x83),
            ("F21", 0x84),
            ("F22", 0x85),
            ("F23", 0x86),
            ("F24", 0x87),
            ("ZERO", 0x30),
            ("ONE", 0x31),
            ("TWO", 0x32),
            ("THREE", 0x33),
            ("FOUR", 0x34),
            ("FIVE", 0x35),
            ("SIX", 0x36),
            ("SEVEN", 0x37),
            ("EIGHT", 0x38),
            ("NINE", 0x39),
            ("A", 0x41),
            ("B", 0x42),
            ("C", 0x43),
            ("D", 0x44),
            ("E", 0x45),
            ("F", 0x46),
            ("G", 0x47),
            ("H", 0x48),
            ("I", 0x49),
            ("J", 0x4A),
            ("K", 0x4B),
            ("L", 0x4C),
            ("M", 0x4D),
            ("N", 0x4E),
            ("O", 0x4F),
            ("P", 0x50),
            ("Q", 0x51),
            ("R", 0x52),
            ("S", 0x53),
            ("T", 0x54),
            ("U", 0x55),
            ("V", 0x56),
            ("W", 0x57),
            ("X", 0x58),
            ("Y", 0x59),
            ("Z", 0x5A),
            ("NUMPAD0", 0x60),
            ("NUMPAD1", 0x61),
            ("NUMPAD2", 0x62),
            ("NUMPAD3", 0x63),
            ("NUMPAD4", 0x64),
            ("NUMPAD5", 0x65),
            ("NUMPAD6", 0x66),
            ("NUMPAD7", 0x67),
            ("NUMPAD8", 0x68),
            ("NUMPAD9", 0x69),
            ("SPACE", 0x20),
            ("TAB", 0x09),
            ("ENTER", 0x0D),
            ("RETURN", 0x0D),
            ("ESCAPE", 0x1B),
            ("BACKSPACE", 0x08),
            ("UP", 0x26),
            ("DOWN", 0x28),
            ("LEFT", 0x25),
            ("RIGHT", 0x27),
            ("LEFT_SHIFT", 0xA0),
            ("RIGHT_SHIFT", 0xA1),
            ("LEFT_CONTROL", 0xA2),
            ("RIGHT_CONTROL", 0xA3),
            ("LEFT_ALT", 0xA4),
            ("RIGHT_ALT", 0xA5),
            ("PAGE_UP", 0x21),
            ("PAGE_DOWN", 0x22),
            ("HOME", 0x24),
            ("END", 0x23),
            ("INSERT", 0x2D),
            ("DELETE", 0x2E),
        ]
    }
}

/// Install the `Key` global on `lua` as a Lua table mapping name → integer
/// virtual-key code.
pub fn load_key_table(lua: &Lua) -> mlua::Result<()> {
    let tbl = lua.create_table()?;
    for (name, vk) in key_table() {
        tbl.set(name, vk)?;
    }
    lua.globals().set("Key", tbl)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_loads_and_resolves_f5() {
        let lua = Lua::new();
        load_key_table(&lua).unwrap();
        let v: u32 = lua.load("return Key.F5").eval().unwrap();
        // Win32 VK_F5 == 0x74.
        assert_eq!(v, 0x74);
    }
}
