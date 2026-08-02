(comment) @comment
(string) @string
(number) @number

(literal "true" @boolean)
(literal "false" @boolean)
(literal "null" @constant.builtin)

(query_definition
  "query" @keyword.declaration
  name: (name) @function)

(fragment_definition
  "fragment" @keyword.declaration
  name: (name) @type
  "on" @keyword)

(filter_definition
  "filter" @keyword.declaration
  name: (name) @function.macro)

(filter_definition "on" @keyword)

(condition_definition
  "condition" @keyword.declaration
  name: (name) @function.macro)

(condition_definition "on" @keyword)

(type_reference
  namespace: (name) @namespace
  name: (name) @type)

(type_reference
  name: (name) @type)

(relation_reference
  namespace: (name) @namespace
  name: (name) @property
  reverse: (name)? @property)

(relation_reference
  name: (name) @property
  reverse: (name)? @property)

(qualified_name
  namespace: (name) @namespace
  name: (name) @property)

(qualified_name
  name: (name) @property)

(scoped_path_segment
  namespace: (name) @namespace
  name: (name) @property
  reverse: (name)? @property)

(scoped_path_segment
  name: (name) @property
  reverse: (name)? @property)

(field_selection
  alias: (relation_reference
    name: (name) @property.definition)
  (#set! priority 110))

(shape_field
  name: (name) @property.definition
  type: (name) @type)

(field_rule
  "field" @keyword
  field: (name) @property
  "where" @keyword)

(apply_rule
  "apply" @keyword)

(apply_rule "where" @keyword)

(filter_assignment
  "filter" @keyword
  filter: (name) @function.macro)

(filter_assignment "when" @keyword)

(top_level_parameter
  name: (variable_name) @variable.parameter)

(structured_parameter
  name: (variable_name) @variable)

(top_level_variable
  name: (variable_name)? @variable.parameter)

(structured_variable
  name: (variable_name)? @variable)

(context_variable
  name: (identifier) @variable.builtin)

(fragment_spread
  fragment: (name) @type)

(binding_item "<-" @operator)

(pipe_transform
  function: (name) @function.builtin)

(pipe_transform "by" @keyword)

(aggregate_group_key
  alias: (name)? @property.definition)

(aggregate_field
  function: (name) @function.builtin)

(aggregate_field
  alias: (name) @property.definition)

(predicate_name
  name: (identifier) @function.call)

(scalar_aggregate_expression
  function: (name) @function.builtin)

(directive
  "@" @punctuation.special)

(directive_name
  namespace: (name) @namespace
  member: (name)? @attribute)

(directive_name
  member: (name) @attribute)

(directive_argument
  name: (name) @variable.parameter)

(dynamic_input
  "on" @keyword
  surface: (dynamic_surface) @constant.builtin)

(where_clause "where" @keyword)
(order_by_clause "order" @keyword "by" @keyword)
(limit_clause "limit" @keyword)
(offset_clause "offset" @keyword)
(sort_direction ["asc" "desc"] @keyword)

(exists_expression "exists" @operator)
(unary_expression "not" @operator)
(binary_expression "not" @operator)
(binary_expression "and" @operator)
(binary_expression "or" @operator)
(binary_expression "in" @operator)
(null_test_expression "not" @operator)
(null_test_expression "is" @operator)
(comparison_operator) @operator
(path_anchor) @operator

[
  "%"
  "$"
  "="
  "?"
  "->"
  "::"
  "."
  "|"
  "..."
] @operator

[
  "("
  ")"
  "["
  "]"
  "{"
  "}"
] @punctuation.bracket

[
  ","
  ":"
] @punctuation.delimiter
