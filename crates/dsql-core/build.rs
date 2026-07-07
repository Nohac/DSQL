fn main() {
    println!("cargo:rerun-if-changed=src/grammar/dsql.llw");
    lelwel::build("src/grammar/dsql.llw");
    // The grammar declares every token; literal spellings become #[token]
    // variants, and only the non-literal tokens need patterns here.
    lelwel::build_lexer(
        "src/grammar/dsql.llw",
        &lelwel::backend::lexer::LexerSpec {
            patterns: &[
                ("Name", r#"r"[A-Za-z_][A-Za-z0-9_]*""#),
                ("String", r##"r#""([^"\\]|\\.)*""#"##),
                ("Number", r#"r"-?[0-9]+(\.[0-9]+)?""#),
                ("Whitespace", r#"r"[ \t\r\n]+""#),
                ("Comment", r##"r"#[^\n\r]*", allow_greedy = true"##),
            ],
        },
    );
}
