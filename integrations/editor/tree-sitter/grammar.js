import { KEYWORDS as K, SYMBOLS as S, TERMINAL_PATTERNS } from "./language-surface.mjs";

const PREC = {
  or: 1,
  and: 2,
  unary: 3,
  nullTest: 4,
  comparison: 5,
  aggregate: 6,
};

export default grammar({
  name: "dsql",

  word: $ => $.identifier,

  extras: $ => [$._whitespace, $.comment],

  supertypes: $ => [$._expression],

  conflicts: $ => [
    [$.fragment_spread, $.relation_reference],
  ],

  rules: {
    document: $ => repeat($._definition),

    _definition: $ => choice(
      $.query_definition,
      $.fragment_definition,
      $.filter_definition,
      $.condition_definition,
    ),

    query_definition: $ => seq(
      K.query,
      field("name", $.name),
      optional(field("inputs", $.query_header)),
      repeat(field("directive", $.directive)),
      field("body", $.selection_set),
    ),

    fragment_definition: $ => seq(
      K.fragment,
      field("name", $.name),
      optional(field("inputs", $.fragment_header)),
      K.on,
      field("target", $.type_reference),
      field("body", $.selection_set),
    ),

    filter_definition: $ => seq(
      K.filter,
      field("name", $.name),
      optional(seq(K.on, field("target", $.policy_target))),
      field("body", $.filter_body),
    ),

    condition_definition: $ => seq(
      K.condition,
      field("name", $.name),
      optional(seq(K.on, field("target", $.policy_target))),
      field("body", $.condition_body),
    ),

    policy_target: $ => choice($.type_reference, $.shape_target),

    shape_target: $ => seq(S.leftBrace, repeat($.shape_field), S.rightBrace),

    shape_field: $ => seq(
      S.current,
      field("name", $.name),
      S.colon,
      field("type", $.name),
      optional(S.comma),
    ),

    filter_body: $ => seq(S.leftBrace, repeat($._filter_rule), S.rightBrace),

    _filter_rule: $ => choice($.apply_rule, $.where_clause, $.field_rule),

    condition_body: $ => seq(S.leftBrace, field("condition", $.where_clause), S.rightBrace),

    apply_rule: $ => prec.right(seq(
      K.apply,
      optional(seq(K.where, field("condition", $._expression))),
    )),

    field_rule: $ => seq(
      K.field,
      field("field", $.name),
      repeat(seq(S.comma, field("field", $.name))),
      K.where,
      field("condition", $._expression),
    ),

    query_header: $ => seq(
      S.leftParenthesis,
      repeat1(choice($.input_refinement, $.filter_assignment)),
      S.rightParenthesis,
    ),

    fragment_header: $ => seq(
      S.leftParenthesis,
      repeat1($.input_refinement),
      S.rightParenthesis,
    ),

    input_refinement: $ => seq(
      field("parameter", choice($.top_level_parameter, $.structured_parameter)),
      choice(
        seq(S.optional, optional(seq(S.assign, field("default", $.default_value)))),
        seq(S.assign, field("default", $.default_value)),
      ),
    ),

    top_level_parameter: $ => seq(S.topLevel, field("name", $.variable_name)),

    structured_parameter: $ => seq(S.structured, field("name", $.variable_name)),

    default_value: $ => choice($.literal, $.default_collection, $.empty_object),

    default_collection: $ => seq(
      S.leftBracket,
      optional(commaSep1($.default_value)),
      optional(S.comma),
      S.rightBracket,
    ),

    empty_object: _ => seq(S.leftBrace, S.rightBrace),

    filter_assignment: $ => seq(
      K.filter,
      field("filter", $.name),
      optional(seq(K.when, field("condition", $._expression))),
    ),

    type_reference: $ => qualified($, "namespace", "name"),

    qualified_name: $ => qualified($, "namespace", "name"),

    relation_reference: $ => seq(
      qualified($, "namespace", "name"),
      optional(seq(S.arrow, field("reverse", $.name))),
    ),

    scoped_path_segment: $ => seq(
      qualified($, "namespace", "name"),
      optional(seq(S.arrow, field("reverse", $.name))),
    ),

    scoped_path: $ => seq(
      field("anchor", $.path_anchor),
      field("segment", $.scoped_path_segment),
      repeat(seq(S.current, field("segment", $.scoped_path_segment))),
    ),

    path_anchor: _ => choice(S.current, S.parent, S.root),

    value_variable: $ => choice(
      $.top_level_variable,
      $.structured_variable,
      $.context_variable,
    ),

    top_level_variable: $ => prec.right(seq(
      S.topLevel,
      optional(field("name", $.variable_name)),
    )),

    structured_variable: $ => prec.right(seq(
      S.structured,
      optional(field("name", $.variable_name)),
    )),

    context_variable: $ => seq(S.structured, S.colon, field("name", $.identifier)),

    dynamic_input: $ => seq(
      field("parameter", $.top_level_variable),
      K.on,
      field("surface", $.dynamic_surface),
    ),

    dynamic_surface: _ => choice(K.selected, K.indexed, K.selectedIndexed, K.searchable),

    comparison_operator: _ => choice(
      S.equal,
      S.notEqual,
      S.greater,
      S.greaterEqual,
      S.less,
      S.lessEqual,
      K.like,
    ),

    operator_variable: $ => seq(
      field("parameter", choice($.top_level_variable, $.structured_variable)),
      S.leftBracket,
      commaSep1($.comparison_operator),
      optional(S.comma),
      S.rightBracket,
    ),

    binary_operator: $ => choice($.comparison_operator, $.operator_variable),

    selection_set: $ => seq(S.leftBrace, repeat($.selection), S.rightBrace),

    selection: $ => choice(
      seq($.field_selection, optional(S.comma)),
      seq($.fragment_spread, optional(S.comma)),
    ),

    fragment_spread: $ => seq(
      S.ellipsis,
      field("fragment", $.name),
      optional(field("bindings", $.binding_list)),
      repeat(field("directive", $.directive)),
    ),

    binding_list: $ => seq(
      S.leftParenthesis,
      $.binding_item,
      repeat(seq(optional(S.comma), $.binding_item)),
      optional(S.comma),
      S.rightParenthesis,
    ),

    binding_item: $ => seq(
      field("target", $.binding_variable),
      optional(seq(S.leftArrow, field("source", $.binding_variable))),
    ),

    binding_variable: $ => choice($.top_level_variable, $.structured_variable),

    field_selection: $ => choice(
      seq(
        optional(field("flatten", S.ellipsis)),
        field("alias", $.relation_reference),
        S.colon,
        field("field", $.relation_reference),
        optional($.field_suffix),
      ),
      seq(
        optional(field("flatten", S.ellipsis)),
        field("field", $.relation_reference),
        optional($.field_suffix),
      ),
    ),

    field_suffix: $ => choice(
      seq($.clause_list, optional($.field_continuation)),
      $.field_continuation,
    ),

    field_continuation: $ => choice(
      $.pipe_transform,
      seq(repeat1($.directive), optional($.selection_set)),
      $.selection_set,
    ),

    pipe_transform: $ => seq(
      S.pipe,
      field("function", $.name),
      optional(seq(
        K.by,
        field("group", $.aggregate_group_key),
        repeat(seq(S.comma, field("group", $.aggregate_group_key))),
        optional(S.comma),
      )),
      field("body", $.aggregate_set),
    ),

    aggregate_group_key: $ => seq(
      optional(seq(field("alias", $.name), S.colon)),
      field("path", $.scoped_path),
    ),

    aggregate_set: $ => seq(S.leftBrace, repeat($.aggregate_field), S.rightBrace),

    aggregate_field: $ => choice(
      seq(
        field("alias", $.name),
        S.colon,
        field("function", $.name),
        optional(field("path", $.scoped_path)),
        optional(S.comma),
      ),
      seq(
        field("function", $.name),
        optional(field("path", $.scoped_path)),
        optional(S.comma),
      ),
    ),

    clause_list: $ => seq(S.leftParenthesis, repeat($._clause), S.rightParenthesis),

    _clause: $ => choice(
      $.filter_assignment,
      $.where_clause,
      $.order_by_clause,
      $.limit_clause,
      $.offset_clause,
    ),

    where_clause: $ => seq(K.where, field("condition", $._expression)),

    order_by_clause: $ => prec.right(seq(
      K.order,
      K.by,
      field("item", $.order_item),
      repeat(seq(S.comma, field("item", $.order_item))),
      optional(S.comma),
    )),

    sort_direction: $ => choice(K.asc, K.desc, $.value_variable),

    order_item: $ => choice(
      $.dynamic_input,
      seq(field("field", $.qualified_name), optional(field("direction", $.sort_direction))),
    ),

    limit_clause: $ => seq(K.limit, field("value", $._expression)),

    offset_clause: $ => seq(K.offset, field("value", $._expression)),

    directive: $ => seq(
      S.at,
      field("name", $.directive_name),
      optional(seq(
        S.leftParenthesis,
        commaSep1($.directive_argument),
        optional(S.comma),
        S.rightParenthesis,
      )),
    ),

    directive_name: $ => choice(
      seq(S.current, field("member", $.name)),
      seq(
        field("namespace", $.name),
        optional(seq(S.current, field("member", $.name))),
      ),
    ),

    directive_argument: $ => seq(
      field("name", $.name),
      S.colon,
      field("value", $._expression),
    ),

    scalar_aggregate_expression: $ => prec(PREC.aggregate, seq(
      field("source", $.scoped_path),
      S.pipe,
      field("function", $.name),
      optional(field("path", $.scoped_path)),
    )),

    collection_literal: $ => seq(
      S.leftBracket,
      optional(commaSep1($._expression)),
      optional(S.comma),
      S.rightBracket,
    ),

    exists_source: $ => seq(
      field("source", choice($.scoped_path, $.qualified_name)),
      optional(seq(
        S.leftParenthesis,
        repeat($.filter_assignment),
        optional($.where_clause),
        S.rightParenthesis,
      )),
    ),

    exists_expression: $ => seq(K.exists, field("source", $.exists_source)),

    _expression: $ => choice(
      $.binary_expression,
      $.null_test_expression,
      $.unary_expression,
      $.parenthesized_expression,
      $.literal,
      $.collection_literal,
      $.exists_expression,
      $.predicate_name,
      $.scalar_aggregate_expression,
      $.scoped_path,
      $.dynamic_input,
      $.value_variable,
    ),

    binary_expression: $ => choice(
      prec.left(PREC.comparison, seq(
        field("left", $._expression),
        field("operator", choice($.binary_operator, K.in, seq(K.not, K.in))),
        field("right", $._expression),
      )),
      prec.left(PREC.and, seq(
        field("left", $._expression),
        K.and,
        field("right", $._expression),
      )),
      prec.left(PREC.or, seq(
        field("left", $._expression),
        K.or,
        field("right", $._expression),
      )),
    ),

    null_test_expression: $ => prec.left(PREC.nullTest, seq(
      field("value", $._expression),
      K.is,
      field("operator", choice(K.null, seq(K.not, K.null))),
    )),

    unary_expression: $ => prec.right(PREC.unary, seq(
      K.not,
      field("operand", $._expression),
    )),

    parenthesized_expression: $ => seq(
      S.leftParenthesis,
      field("expression", $._expression),
      S.rightParenthesis,
    ),

    predicate_name: $ => field("name", $.identifier),

    literal: $ => choice($.string, $.number, K.true, K.false, K.null),

    name: $ => choice($.identifier, $.contextual_name),

    contextual_name: _ => choice(
      K.not,
      K.in,
      K.is,
      K.exists,
      K.filter,
      K.condition,
      K.apply,
      K.when,
      K.field,
      K.selected,
      K.indexed,
      K.selectedIndexed,
      K.searchable,
    ),

    variable_name: $ => choice(
      $.identifier,
      $.contextual_name,
      K.query,
      K.fragment,
      K.where,
      K.order,
      K.by,
      K.limit,
      K.offset,
      K.asc,
      K.desc,
    ),

    identifier: _ => token(new RegExp(TERMINAL_PATTERNS.Name)),
    string: _ => token(new RegExp(TERMINAL_PATTERNS.String)),
    number: _ => token(new RegExp(TERMINAL_PATTERNS.Number)),
    _whitespace: _ => token(new RegExp(TERMINAL_PATTERNS.Whitespace)),
    comment: _ => token(new RegExp(TERMINAL_PATTERNS.Comment)),
  },
});

function qualified($, namespaceField, nameField) {
  return seq(
    optional(seq(field(namespaceField, $.name), S.qualified)),
    field(nameField, $.name),
  );
}

function commaSep1(rule) {
  return seq(rule, repeat(seq(S.comma, rule)));
}
