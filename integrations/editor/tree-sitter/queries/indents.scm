[
  (shape_target)
  (filter_body)
  (condition_body)
  (query_header)
  (fragment_header)
  (default_collection)
  (empty_object)
  (operator_variable)
  (selection_set)
  (binding_list)
  (aggregate_set)
  (clause_list)
  (directive)
  (collection_literal)
  (exists_source)
  (parenthesized_expression)
] @indent.begin

; Continuation lists have no enclosing delimiter of their own, but their later
; items still belong one level below the clause or transform that introduced them.
[
  (order_by_clause)
  (pipe_transform)
] @indent.begin

; Several unfinished nested constructs recover as one flat ERROR node. Preserve
; one useful indentation level until the parser can rebuild named containers.
(ERROR
  [
    "{"
    "("
    "["
  ]) @indent.begin

[
  "}"
  ")"
  "]"
] @indent.branch

[
  "}"
  ")"
  "]"
] @indent.end
