//! Byte-offset ↔ LSP position mapping. Spans are byte ranges end to end;
//! UTF-16 line/character positions exist only at this protocol boundary.

use dsql_core::service::{SemanticToken as CoreSemanticToken, SemanticTokenKind};
use ropey::{LineType, Rope};
use tower_lsp_server::ls_types::{
    Position, SemanticToken, SemanticTokenType, SemanticTokensLegend,
};

pub(crate) fn byte_to_position(rope: &Rope, byte: usize) -> Position {
    let byte = byte.min(rope.len());
    let line = rope.byte_to_line_idx(byte, LineType::LF_CR);
    let line_start = rope.line_to_byte_idx(line, LineType::LF_CR);
    let character = rope.slice(line_start..byte).len_utf16();
    Position::new(line as u32, character as u32)
}

pub(crate) fn position_to_byte(rope: &Rope, position: Position) -> usize {
    let line_count = rope.len_lines(LineType::LF_CR);
    let line = (position.line as usize).min(line_count.saturating_sub(1));
    let line_start = rope.line_to_byte_idx(line, LineType::LF_CR);
    let mut line_end = if line + 1 < line_count {
        rope.line_to_byte_idx(line + 1, LineType::LF_CR)
    } else {
        rope.len()
    };
    // Per the LSP spec, a character past the end of the line clamps to the
    // line length *before* the break — the slice must exclude the
    // terminator or past-EOL positions map onto the next line.
    let tail_start = line_start.max(line_end.saturating_sub(2));
    let tail = rope.slice(tail_start..line_end).to_string();
    if tail.ends_with('\n') {
        line_end -= 1;
        if tail.ends_with("\r\n") {
            line_end -= 1;
        }
    }
    let line_slice = rope.slice(line_start..line_end);
    let utf16 = (position.character as usize).min(line_slice.len_utf16());
    line_start + line_slice.utf16_to_byte_idx(utf16)
}

/// Token types in [`semantic_token_type`] index order.
pub(crate) fn semantic_tokens_legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: vec![
            SemanticTokenType::NAMESPACE,
            SemanticTokenType::CLASS,
            SemanticTokenType::new("relation"),
            SemanticTokenType::PROPERTY,
            SemanticTokenType::new("fragment"),
            SemanticTokenType::new("alias"),
            SemanticTokenType::new("policy"),
        ],
        token_modifiers: Vec::new(),
    }
}

fn semantic_token_type(kind: SemanticTokenKind) -> u32 {
    match kind {
        SemanticTokenKind::Schema => 0,
        SemanticTokenKind::Table => 1,
        SemanticTokenKind::Relation => 2,
        SemanticTokenKind::Column => 3,
        SemanticTokenKind::Fragment => 4,
        SemanticTokenKind::Alias => 5,
        SemanticTokenKind::Policy => 6,
    }
}

/// Delta-encodes span-sorted tokens per the LSP wire format; multi-line
/// spans can't be expressed and are dropped.
pub(crate) fn encode_semantic_tokens(
    rope: &Rope,
    tokens: &[CoreSemanticToken],
) -> Vec<SemanticToken> {
    let mut encoded = Vec::new();
    let mut previous_line = 0;
    let mut previous_start = 0;
    for token in tokens {
        let start = byte_to_position(rope, token.span.start);
        let end = byte_to_position(rope, token.span.end);
        if start.line != end.line || start.character >= end.character {
            continue;
        }
        let delta_line = start.line - previous_line;
        let delta_start = if delta_line == 0 {
            start.character - previous_start
        } else {
            start.character
        };
        encoded.push(SemanticToken {
            delta_line,
            delta_start,
            length: end.character - start.character,
            token_type: semantic_token_type(token.kind),
            token_modifiers_bitset: 0,
        });
        previous_line = start.line;
        previous_start = start.character;
    }
    encoded
}
