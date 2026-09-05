; HTML captures retained from tree-sitter-html.
(doctype) @constant
(tag_name) @tag
(erroneous_end_tag_name) @tag.error
(attribute_name) @tag.attribute
(attribute_value) @string
(comment) @comment

[
  "<"
  ">"
  "</"
  "/>"
] @tag.delimiter
"=" @operator

; Radiant template constructs.
(delimiter) @punctuation.special
(escaped_brace) @string.escape
(radiant_comment) @comment
(unparsed_content) @string.special

(parameter_declaration
  type: (type_identifier) @type
  name: (identifier) @variable.parameter)

(section_open
  name: (section_name) @keyword)
(section_close
  name: (section_name) @keyword)

(named_argument
  name: (identifier) @variable.parameter)
(template_reference) @string.special.path

(identifier) @variable
(namespace_expression
  namespace: (identifier) @module)
(namespace_expression
  name: (identifier) @function)
(member_expression
  member: (identifier) @property)
(call_expression
  function: (expression (identifier) @function.call))

(string) @string
(integer) @number
(float) @number.float
(boolean) @boolean
(null) @constant.builtin

(parameter_declaration operator: _ @operator)
(named_argument operator: _ @operator)
(unary_expression operator: _ @operator)
(binary_expression operator: _ @operator)
(safe_expression operator: _ @operator)

["(" ")" "[" "]"] @punctuation.bracket
["." "," ":"] @punctuation.delimiter
