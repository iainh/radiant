use radiant_compiler::Span;
use tower_lsp::lsp_types::{Position, Range};

/// Converts between compiler UTF-8 byte offsets and LSP UTF-16 positions.
#[derive(Debug)]
pub struct LineIndex {
    text: String,
    line_starts: Vec<usize>,
}

impl LineIndex {
    #[must_use]
    pub fn new(text: &str) -> Self {
        let mut line_starts = vec![0];
        for (offset, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push(offset + 1);
            }
        }
        Self {
            text: text.into(),
            line_starts,
        }
    }

    #[must_use]
    pub fn byte_to_position(&self, byte: usize) -> Position {
        let mut byte = byte.min(self.text.len());
        while !self.text.is_char_boundary(byte) {
            byte -= 1;
        }
        let line = self.line_starts.partition_point(|start| *start <= byte) - 1;
        let start = self.line_starts[line];
        let end = self.line_content_end(line);
        let byte = byte.min(end);
        let character = self.text[start..byte]
            .chars()
            .map(char::len_utf16)
            .sum::<usize>();
        Position::new(line as u32, character as u32)
    }

    #[must_use]
    pub fn position_to_byte(&self, position: Position) -> usize {
        let line = (position.line as usize).min(self.line_starts.len() - 1);
        if position.line as usize >= self.line_starts.len() {
            return self.text.len();
        }
        let start = self.line_starts[line];
        let end = self.line_content_end(line);
        let mut utf16 = 0_u32;
        for (relative, character) in self.text[start..end].char_indices() {
            let next = utf16 + character.len_utf16() as u32;
            if next > position.character {
                return start + relative;
            }
            utf16 = next;
        }
        end
    }

    #[must_use]
    pub fn span_to_range(&self, span: Span) -> Range {
        Range::new(
            self.byte_to_position(span.start),
            self.byte_to_position(span.end),
        )
    }

    fn line_content_end(&self, line: usize) -> usize {
        let mut end = self
            .line_starts
            .get(line + 1)
            .map_or(self.text.len(), |start| start - 1);
        if self.text.as_bytes().get(end.wrapping_sub(1)) == Some(&b'\r') {
            end -= 1;
        }
        end
    }
}

#[cfg(test)]
mod tests {
    use tower_lsp::lsp_types::Position;

    use super::LineIndex;

    #[test]
    fn converts_ascii_and_multiline_positions() {
        let lines = LineIndex::new("abc\ndef");

        assert_eq!(lines.byte_to_position(2), Position::new(0, 2));
        assert_eq!(lines.byte_to_position(5), Position::new(1, 1));
        assert_eq!(lines.position_to_byte(Position::new(0, 2)), 2);
        assert_eq!(lines.position_to_byte(Position::new(1, 1)), 5);
    }

    #[test]
    fn converts_multibyte_and_astral_characters_as_utf16() {
        let lines = LineIndex::new("é😀x");

        assert_eq!(lines.byte_to_position("é😀".len()), Position::new(0, 3));
        assert_eq!(lines.position_to_byte(Position::new(0, 1)), "é".len());
        assert_eq!(lines.position_to_byte(Position::new(0, 3)), "é😀".len());
    }

    #[test]
    fn clamps_offsets_inside_utf8_and_utf16_characters() {
        let lines = LineIndex::new("é😀x");

        assert_eq!(lines.byte_to_position(1), Position::new(0, 0));
        assert_eq!(lines.byte_to_position(4), Position::new(0, 1));
        assert_eq!(lines.position_to_byte(Position::new(0, 2)), "é".len());
    }

    #[test]
    fn excludes_crlf_from_line_contents() {
        let lines = LineIndex::new("ab\r\ncd");

        assert_eq!(lines.byte_to_position(2), Position::new(0, 2));
        assert_eq!(lines.byte_to_position(3), Position::new(0, 2));
        assert_eq!(lines.byte_to_position(4), Position::new(1, 0));
        assert_eq!(lines.position_to_byte(Position::new(0, 99)), 2);
    }

    #[test]
    fn clamps_out_of_range_positions_and_offsets() {
        let lines = LineIndex::new("a\nb");

        assert_eq!(lines.byte_to_position(99), Position::new(1, 1));
        assert_eq!(lines.position_to_byte(Position::new(0, 99)), 1);
        assert_eq!(lines.position_to_byte(Position::new(99, 0)), 3);
    }

    #[test]
    fn handles_empty_and_trailing_empty_lines() {
        let empty = LineIndex::new("");
        assert_eq!(empty.byte_to_position(0), Position::new(0, 0));
        assert_eq!(empty.position_to_byte(Position::new(0, 0)), 0);

        let trailing = LineIndex::new("a\n");
        assert_eq!(trailing.byte_to_position(2), Position::new(1, 0));
        assert_eq!(trailing.position_to_byte(Position::new(1, 4)), 2);
    }
}
