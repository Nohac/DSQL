use dsql_frontend::{SemanticTokenInfo, SemanticTokenKind};
use ropey::{LineType, Rope};
use tower_lsp_server::ls_types::{Position, SemanticToken};

pub(crate) fn encode_semantic_tokens(
    rope: &Rope,
    tokens: &[SemanticTokenInfo],
) -> Vec<SemanticToken> {
    let mut encoded = Vec::new();
    let mut previous_line = 0;
    let mut previous_start = 0;
    for token in tokens {
        let start = byte_to_position(rope, token.range.start as usize);
        let end = byte_to_position(rope, token.range.end as usize);
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

fn semantic_token_type(kind: SemanticTokenKind) -> u32 {
    match kind {
        SemanticTokenKind::Schema => 0,
        SemanticTokenKind::Table => 1,
        SemanticTokenKind::Relation => 2,
        SemanticTokenKind::Column => 3,
        SemanticTokenKind::Fragment => 4,
        SemanticTokenKind::Alias => 5,
    }
}

pub(crate) fn byte_to_position(rope: &Rope, byte: usize) -> Position {
    let byte = byte.min(rope.len());
    let line = rope.byte_to_line_idx(byte, LineType::LF_CR);
    let line_start = rope.line_to_byte_idx(line, LineType::LF_CR);
    let character = rope.slice(line_start..byte).len_utf16();
    Position::new(line as u32, character as u32)
}
