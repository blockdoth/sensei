use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::state::FocussedInput;

/// Represents the settings used for Wi-Fi frame spamming.
/// Includes source and destination MAC addresses, number of repetitions,
/// and pause duration in milliseconds.
#[derive(Clone, Debug, PartialEq)]
pub struct SpamSettings {
    pub src_mac: [u8; 6],
    pub dst_mac: [u8; 6],
    pub n_reps: u32,
    pub pause_ms: u32,
}

impl SpamSettings {
    /// Formats the current spam settings into a TUI-compatible set of lines,
    /// applying style and highlighting based on the currently focused input.
    ///
    /// # Arguments
    /// * `focus` - The input field that currently has focus, used for styling.
    ///
    /// # Returns
    /// A vector of `Line` elements that can be rendered in a terminal UI.
    pub fn format(&self, focus: FocussedInput) -> Vec<Line> {
        let (src_mac_f, dst_mac_f, n_reps_f, pause_ms_f) = match focus {
            FocussedInput::SrcMac(i) => (Some(i), None, None, None),
            FocussedInput::DstMac(i) => (None, Some(i), None, None),
            FocussedInput::Reps(i) => (None, None, Some(i), None),
            FocussedInput::PauseMs(i) => (None, None, None, Some(i)),
            FocussedInput::None => (None, None, None, None),
        };
        let src_mac_s = self.format_mac("Src MAC:  ".into(), self.src_mac, src_mac_f);
        let dst_mac_s = self.format_mac("Dest MAC: ".into(), self.dst_mac, dst_mac_f);
        let n_reps_s = self.format_text("  Reps:  ".into(), self.n_reps, "".into(), n_reps_f);
        let pause_ms_s = self.format_text("  Pause: ".into(), self.pause_ms, "ms".into(), pause_ms_f);

        vec![
            Line::from(src_mac_s.into_iter().chain(n_reps_s).collect::<Vec<_>>()),
            Line::from(dst_mac_s.into_iter().chain(pause_ms_s).collect::<Vec<_>>()),
        ]
    }

    /// Formats a numeric field (e.g., repetitions or pause duration) into styled spans,
    /// allowing for visual cursor highlighting of a digit.
    ///
    /// # Arguments
    /// * `header` - The label shown before the value (e.g., "Reps:").
    /// * `value` - The actual numeric value to be rendered.
    /// * `unit` - A string to be appended after the value (e.g., "ms").
    /// * `selected_index` - The index of the digit that is currently selected.
    ///
    /// # Returns
    /// A vector of `Span` objects with appropriate styling.
    fn format_text<T: ToString>(&self, header: String, value: T, unit: String, selected_index: Option<usize>) -> Vec<Span> {
        let value_str = value.to_string();
        let value_str_len = value_str.len();
        let mut spans = vec![Span::raw(header)];
        if selected_index.is_some() {
            for (i, ch) in value_str.chars().enumerate() {
                let mut style = Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD);
                if selected_index == Some(i) {
                    style = style.add_modifier(Modifier::UNDERLINED);
                }
                spans.push(Span::styled(ch.to_string(), style));
            }
        } else {
            spans.push(Span::raw(value_str));
        };

        if selected_index == Some(value_str_len) {
            spans.push(Span::styled(
                " ",
                Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::UNDERLINED),
            ));
        }
        spans.push(Span::raw(unit));
        spans
    }

    /// Formats a MAC address as a set of styled spans for TUI rendering.
    /// Supports highlighting a specific nibble based on cursor position.
    ///
    /// # Arguments
    /// * `header` - Label before the MAC (e.g., "Src MAC:").
    /// * `mac` - A 6-byte MAC address.
    /// * `selected_index` - The nibble index (0 to 11) currently focused.
    ///
    /// # Returns
    /// A vector of `Span` objects with appropriate styling.
    fn format_mac(&self, header: String, mac: [u8; 6], selected_index: Option<usize>) -> Vec<Span> {
        let mut spans = vec![Span::raw(header)];
        for (idx, byte) in mac.iter().enumerate() {
            let hex_str = format!("{byte:02X}");
            let (first, second) = hex_str.split_at(1);
            if selected_index.map(|i| i / 2) == Some(idx) {
                let mut style_1 = Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD);
                let mut style_2 = style_1;
                if selected_index.unwrap_or(0) % 2 == 0 {
                    style_1 = style_1.add_modifier(Modifier::UNDERLINED);
                } else {
                    style_2 = style_1.add_modifier(Modifier::UNDERLINED);
                }
                spans.push(Span::styled(first.to_string(), style_1));
                spans.push(Span::styled(second.to_string(), style_2));
            } else {
                spans.push(Span::styled(format!("{first}{second}"), Style::default()));
            };

            if idx != mac.len() - 1 {
                spans.push(Span::raw(":"));
            }
        }

        spans
    }

    /// Updates a single byte of a MAC address by modifying one nibble (half-byte)
    /// based on cursor position and character input.
    ///
    /// # Arguments
    /// * `old` - The original byte value.
    /// * `chr` - The new character input (hex digit).
    /// * `index` - The nibble index to update (even = high nibble, odd = low nibble).
    ///
    /// # Returns
    /// A new `u8` with the updated nibble.    
    pub fn update_mac(old: u8, chr: char, index: usize) -> u8 {
        let ascii_char: u8 = chr as u8;
        let u4 = match ascii_char {
            b'0'..=b'9' => ascii_char - b'0',
            b'a'..=b'f' => ascii_char - b'a' + 10,
            b'A'..=b'F' => ascii_char - b'A' + 10,
            _ => 0,
        };
        if index % 2 == 0 {
            (u4 << 4) | (old & 0x0F) // set upper nibble
        } else {
            (old & 0xF0) | u4 // set lower nibble
        }
    }

    /// Modifies a digit at a specified index in a `u32` number as if the number were a string.
    ///
    /// # Arguments
    /// * `number` - The original number.
    /// * `index` - The index of the digit to replace or remove.
    /// * `replacement` - An optional character to insert. If `None`, the digit is removed.
    ///
    /// # Returns
    /// The new number after modification, or 0 on failure.
    pub fn modify_digit_at_index(number: u32, index: usize, replacement: Option<char>) -> u32 {
        let mut chars: Vec<char> = number.to_string().chars().collect();
        match replacement {
            Some(ch) if ch.is_ascii_digit() => {
                // Double check just to be sure
                if index >= chars.len() {
                    chars.push(ch);
                } else {
                    chars[index] = ch;
                }
            }
            None => {
                if index < chars.len() {
                    chars.remove(index);
                } else {
                    chars.pop();
                };
            }
            _ => return 0,
        }
        chars.into_iter().collect::<String>().parse::<u32>().unwrap_or(0)
    }
}

impl Default for SpamSettings {
    fn default() -> Self {
        Self {
            src_mac: [0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC],
            dst_mac: [0xB4, 0x82, 0xC5, 0x58, 0xA1, 0xC0],
            n_reps: 10,
            pause_ms: 10,
        }
    }
}

impl FocussedInput {
    /// Moves focus to the next logical input field to the right.
    /// Cycles from SrcMac → Reps → DstMac → PauseMs → SrcMac.
    pub fn tab_right(self) -> Self {
        match self {
            FocussedInput::SrcMac(i) if i > 9 => FocussedInput::Reps(0),
            FocussedInput::SrcMac(i) => FocussedInput::SrcMac(i + 2),
            FocussedInput::DstMac(i) if i > 9 => FocussedInput::PauseMs(0),
            FocussedInput::DstMac(i) => FocussedInput::DstMac(i + 2),
            FocussedInput::Reps(_) => FocussedInput::DstMac(0),
            FocussedInput::PauseMs(_) => FocussedInput::SrcMac(0),
            FocussedInput::None => FocussedInput::SrcMac(0),
        }
    }

    /// Moves focus to the next logical input field to the left.
    /// Cycles in reverse of `tab_right`.
    pub fn tab_left(self) -> Self {
        match self {
            FocussedInput::SrcMac(i) if i >= 2 => FocussedInput::SrcMac(i - 2),
            FocussedInput::SrcMac(_) => FocussedInput::PauseMs(0),
            FocussedInput::DstMac(i) if i >= 2 => FocussedInput::DstMac(i - 2),
            FocussedInput::DstMac(_) => FocussedInput::Reps(0),
            FocussedInput::Reps(_) => FocussedInput::SrcMac(11),
            FocussedInput::PauseMs(_) => FocussedInput::DstMac(11),
            FocussedInput::None => FocussedInput::SrcMac(0),
        }
    }
    /// Moves the cursor one position to the left within the current input field.
    /// If at the start of a field, moves to the end of the previous logical field.
    pub fn cursor_left(self) -> Self {
        match self {
            FocussedInput::SrcMac(0) => FocussedInput::SrcMac(0),
            FocussedInput::SrcMac(i) => FocussedInput::SrcMac(i - 1),
            FocussedInput::DstMac(0) => FocussedInput::DstMac(0),
            FocussedInput::DstMac(i) => FocussedInput::DstMac(i - 1),
            FocussedInput::Reps(i) if i > 0 => FocussedInput::Reps(i - 1),
            FocussedInput::Reps(_) => FocussedInput::SrcMac(11),
            FocussedInput::PauseMs(i) if i > 0 => FocussedInput::PauseMs(i - 1),
            FocussedInput::PauseMs(_) => FocussedInput::DstMac(11),
            FocussedInput::None => FocussedInput::SrcMac(0),
        }
    }
    /// Moves the cursor one position to the right within the current input field.
    /// If at the end of a field, moves to the beginning of the next logical field.
    pub fn cursor_right(self) -> Self {
        match self {
            FocussedInput::SrcMac(11) => FocussedInput::Reps(0),
            FocussedInput::SrcMac(i) => FocussedInput::SrcMac(i + 1),
            FocussedInput::DstMac(11) => FocussedInput::PauseMs(0),
            FocussedInput::DstMac(i) => FocussedInput::DstMac(i + 1),
            FocussedInput::Reps(i) if i < 9 => FocussedInput::Reps(i + 1),
            FocussedInput::Reps(i) => FocussedInput::Reps(i),
            FocussedInput::PauseMs(i) if i < 9 => FocussedInput::PauseMs(i + 1),
            FocussedInput::PauseMs(i) => FocussedInput::PauseMs(i),
            FocussedInput::None => FocussedInput::SrcMac(0),
        }
    }
    /// Moves the focus vertically up (e.g., from DstMac to SrcMac).
    /// Keeps the cursor in the same relative position when possible.
    pub fn cursor_up(self) -> Self {
        match self {
            FocussedInput::SrcMac(i) => FocussedInput::SrcMac(i),
            FocussedInput::DstMac(i) => FocussedInput::SrcMac(i),
            FocussedInput::Reps(i) => FocussedInput::Reps(i),
            FocussedInput::PauseMs(i) => FocussedInput::Reps(i),
            FocussedInput::None => FocussedInput::SrcMac(0),
        }
    }

    /// Moves the focus vertically down (e.g., from SrcMac to DstMac).
    /// Keeps the cursor in the same relative position when possible.
    pub fn cursor_down(self) -> Self {
        match self {
            FocussedInput::SrcMac(i) => FocussedInput::DstMac(i),
            FocussedInput::DstMac(i) => FocussedInput::DstMac(i),
            FocussedInput::Reps(i) => FocussedInput::PauseMs(i),
            FocussedInput::PauseMs(i) => FocussedInput::PauseMs(i),
            FocussedInput::None => FocussedInput::SrcMac(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spam_settings_default() {
        let default_settings = SpamSettings::default();
        assert_eq!(default_settings.src_mac, [0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC]);
        assert_eq!(default_settings.dst_mac, [0xB4, 0x82, 0xC5, 0x58, 0xA1, 0xC0]);
        assert_eq!(default_settings.n_reps, 10);
        assert_eq!(default_settings.pause_ms, 10);
    }

    #[test]
    fn test_update_mac() {
        // Test updating the upper nibble
        assert_eq!(SpamSettings::update_mac(0xAB, 'F', 0), 0xFB); // F replaces A
        // Test updating the lower nibble
        assert_eq!(SpamSettings::update_mac(0xAB, '0', 1), 0xA0); // 0 replaces B
        // Test with lowercase hex
        assert_eq!(SpamSettings::update_mac(0x34, 'c', 0), 0xc4); // c replaces 3
        // Test with invalid char (should result in 0 for the nibble)
        assert_eq!(SpamSettings::update_mac(0x56, 'x', 0), 0x06); // x is invalid, 0 replaces 5
        assert_eq!(SpamSettings::update_mac(0x56, 'x', 1), 0x50); // x is invalid, 0 replaces 6
    }

    #[test]
    fn test_modify_digit_at_index() {
        // Replace digit
        assert_eq!(SpamSettings::modify_digit_at_index(123, 0, Some('9')), 923);
        assert_eq!(SpamSettings::modify_digit_at_index(123, 1, Some('9')), 193);
        assert_eq!(SpamSettings::modify_digit_at_index(123, 2, Some('9')), 129);

        // Remove digit (by replacing with None)
        assert_eq!(SpamSettings::modify_digit_at_index(123, 0, None), 23);
        assert_eq!(SpamSettings::modify_digit_at_index(123, 1, None), 13);
        assert_eq!(SpamSettings::modify_digit_at_index(123, 2, None), 12);
        assert_eq!(SpamSettings::modify_digit_at_index(7, 0, None), 0); // Removing last digit results in 0

        // Index out of bounds
        assert_eq!(SpamSettings::modify_digit_at_index(123, 0, Some('a')), 0);

        // Test with 0
        assert_eq!(SpamSettings::modify_digit_at_index(0, 0, Some('5')), 5);
        assert_eq!(SpamSettings::modify_digit_at_index(10, 1, None), 1);
    }

    #[test]
    fn test_focussed_input_tab_right() {
        assert_eq!(FocussedInput::None.tab_right(), FocussedInput::SrcMac(0));
        assert_eq!(FocussedInput::SrcMac(0).tab_right(), FocussedInput::SrcMac(2));
        assert_eq!(FocussedInput::SrcMac(10).tab_right(), FocussedInput::Reps(0)); // Cycle to Reps
        assert_eq!(FocussedInput::Reps(0).tab_right(), FocussedInput::DstMac(0));
        assert_eq!(FocussedInput::DstMac(0).tab_right(), FocussedInput::DstMac(2));
        assert_eq!(FocussedInput::DstMac(10).tab_right(), FocussedInput::PauseMs(0)); // Cycle to PauseMs
        assert_eq!(FocussedInput::PauseMs(0).tab_right(), FocussedInput::SrcMac(0)); // Cycle back to SrcMac
    }

    #[test]
    fn test_focussed_input_tab_left() {
        assert_eq!(FocussedInput::None.tab_left(), FocussedInput::SrcMac(0)); // Default to SrcMac

        // SrcMac
        assert_eq!(FocussedInput::SrcMac(2).tab_left(), FocussedInput::SrcMac(0));
        assert_eq!(FocussedInput::SrcMac(1).tab_left(), FocussedInput::PauseMs(0)); // Cycle to PauseMs
        assert_eq!(FocussedInput::SrcMac(0).tab_left(), FocussedInput::PauseMs(0)); // Cycle to PauseMs

        // Reps
        assert_eq!(FocussedInput::Reps(0).tab_left(), FocussedInput::SrcMac(11)); // Cycle to end of SrcMac

        // DstMac
        assert_eq!(FocussedInput::DstMac(2).tab_left(), FocussedInput::DstMac(0));
        assert_eq!(FocussedInput::DstMac(1).tab_left(), FocussedInput::Reps(0));   // Cycle to Reps
        assert_eq!(FocussedInput::DstMac(0).tab_left(), FocussedInput::Reps(0));   // Cycle to Reps
        assert_eq!(FocussedInput::DstMac(11).tab_left(), FocussedInput::DstMac(9));


        // PauseMs
        assert_eq!(FocussedInput::PauseMs(0).tab_left(), FocussedInput::DstMac(11)); // Cycle to end of DstMac
    }

    #[test]
    fn test_focussed_input_cursor_left_right() {
        // --- cursor_left ---
        assert_eq!(FocussedInput::None.cursor_left(), FocussedInput::SrcMac(0));

        // SrcMac
        assert_eq!(FocussedInput::SrcMac(1).cursor_left(), FocussedInput::SrcMac(0));
        assert_eq!(FocussedInput::SrcMac(0).cursor_left(), FocussedInput::SrcMac(0)); // Stays at beginning

        // Reps
        assert_eq!(FocussedInput::Reps(1).cursor_left(), FocussedInput::Reps(0));
        assert_eq!(FocussedInput::Reps(0).cursor_left(), FocussedInput::SrcMac(11)); // Jump to end of SrcMac

        // DstMac
        assert_eq!(FocussedInput::DstMac(1).cursor_left(), FocussedInput::DstMac(0));
        assert_eq!(FocussedInput::DstMac(0).cursor_left(), FocussedInput::DstMac(0)); // Stays at beginning

        // PauseMs
        assert_eq!(FocussedInput::PauseMs(1).cursor_left(), FocussedInput::PauseMs(0));
        assert_eq!(FocussedInput::PauseMs(0).cursor_left(), FocussedInput::DstMac(11)); // Jump to end of DstMac

        // --- cursor_right ---
        assert_eq!(FocussedInput::None.cursor_right(), FocussedInput::SrcMac(0));

        // SrcMac
        assert_eq!(FocussedInput::SrcMac(0).cursor_right(), FocussedInput::SrcMac(1));
        assert_eq!(FocussedInput::SrcMac(11).cursor_right(), FocussedInput::Reps(0)); // Jump to start of Reps

        // Reps
        assert_eq!(FocussedInput::Reps(0).cursor_right(), FocussedInput::Reps(1));
        assert_eq!(FocussedInput::Reps(8).cursor_right(), FocussedInput::Reps(9));
        assert_eq!(FocussedInput::Reps(9).cursor_right(), FocussedInput::Reps(9)); // Stays at index 9 (max for navigation)

        // DstMac
        assert_eq!(FocussedInput::DstMac(0).cursor_right(), FocussedInput::DstMac(1));
        assert_eq!(FocussedInput::DstMac(11).cursor_right(), FocussedInput::PauseMs(0)); // Jump to start of PauseMs

        // PauseMs
        assert_eq!(FocussedInput::PauseMs(0).cursor_right(), FocussedInput::PauseMs(1));
        assert_eq!(FocussedInput::PauseMs(8).cursor_right(), FocussedInput::PauseMs(9));
        assert_eq!(FocussedInput::PauseMs(9).cursor_right(), FocussedInput::PauseMs(9)); // Stays at index 9 (max for navigation)
    }

    #[test]
    fn test_focussed_input_cursor_up_down() {
        // From SrcMac
        assert_eq!(FocussedInput::SrcMac(0).cursor_up(), FocussedInput::SrcMac(0)); // Stays on SrcMac
        assert_eq!(FocussedInput::SrcMac(0).cursor_down(), FocussedInput::DstMac(0)); // SrcMac -> DstMac

        // From DstMac
        assert_eq!(FocussedInput::DstMac(5).cursor_up(), FocussedInput::SrcMac(5));   // DstMac -> SrcMac
        assert_eq!(FocussedInput::DstMac(5).cursor_down(), FocussedInput::DstMac(5)); // Stays on DstMac

        // From Reps
        assert_eq!(FocussedInput::Reps(0).cursor_up(), FocussedInput::Reps(0));     // Stays on Reps
        assert_eq!(FocussedInput::Reps(0).cursor_down(), FocussedInput::PauseMs(0)); // Reps -> PauseMs

        // From PauseMs
        assert_eq!(FocussedInput::PauseMs(0).cursor_up(), FocussedInput::Reps(0));    // PauseMs -> Reps
        assert_eq!(FocussedInput::PauseMs(0).cursor_down(), FocussedInput::PauseMs(0)); // Stays on PauseMs
    }

}
