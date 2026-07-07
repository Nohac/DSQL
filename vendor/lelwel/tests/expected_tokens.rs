use lelwel::frontend::lexer::Token;
use lelwel::frontend::parser::Parser;

#[test]
fn parse_collects_structured_expected_tokens() {
    let source = "token A";
    let mut diagnostics = Vec::new();
    let cst = Parser::new(source, &mut diagnostics).parse(&mut diagnostics);
    let tokens = cst
        .expected_tokens()
        .iter()
        .map(|expected| expected.token)
        .collect::<Vec<_>>();

    assert!(tokens.contains(&Token::Equal));
    assert!(tokens.contains(&Token::Id));
    assert!(tokens.contains(&Token::Semi));
}
