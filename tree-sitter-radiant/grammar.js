/**
 * @file Tree-sitter grammar for Radiant templates
 * @license MIT OR Apache-2.0
 */

/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

const HTML = require('tree-sitter-html/grammar');

const PREC = {
  elvis: 1,
  or: 2,
  and: 3,
  equality: 4,
  comparison: 5,
  additive: 6,
  multiplicative: 7,
  unary: 8,
  postfix: 9,
};

module.exports = grammar(HTML, {
  name: 'radiant',

  rules: {
    _node: ($, previous) => choice($._radiant_node, previous),

    _radiant_node: $ => choice(
      $.output_expression,
      $.parameter_declaration,
      $.radiant_comment,
      $.unparsed_block,
      $.section_open,
      $.section_close,
      $.escaped_brace,
    ),

    // HTML text must stop at a Radiant delimiter so template constructs win
    // over the inherited HTML text token.
    text: _ => choice(
      /[^<>&{\\\s]([^<>&{\\]*[^<>&{\\\s])?/,
      /\{[^A-Za-z0-9_!|#/@]/,
      '\\',
    ),

    escaped_brace: _ => /\\[{}]/,

    attribute: $ => seq(
      $.attribute_name,
      optional(seq(
        '=',
        choice(
          $.attribute_value,
          $.quoted_attribute_value,
          $.output_expression,
        ),
      )),
    ),

    attribute_value: _ => /[^<>"'=\s{]+/,

    quoted_attribute_value: $ => choice(
      seq(
        '\'',
        repeat(choice(
          alias(/[^'{]+/, $.attribute_value),
          $._radiant_node,
        )),
        '\'',
      ),
      seq(
        '"',
        repeat(choice(
          alias(/[^"{]+/, $.attribute_value),
          $._radiant_node,
        )),
        '"',
      ),
    ),

    output_expression: $ => seq(
      alias('{', $.delimiter),
      field('expression', $.expression),
      alias('}', $.delimiter),
    ),

    parameter_declaration: $ => seq(
      alias('{@', $.delimiter),
      field('type', $.type_identifier),
      field('name', $.identifier),
      optional(seq(
        field('operator', '='),
        field('default', $.expression),
      )),
      alias('}', $.delimiter),
    ),

    radiant_comment: $ => seq(
      alias('{!', $.delimiter),
      optional(alias(token(prec(-1, /([^!]|![^}])+/)), $.comment_content)),
      alias('!}', $.delimiter),
    ),

    unparsed_block: $ => seq(
      alias(token(/\{\|+/), $.delimiter),
      optional(alias(token(prec(-1, /([^|]|\|[^}])+/)), $.unparsed_content)),
      alias(token(/\|+\}/), $.delimiter),
    ),

    section_open: $ => seq(
      alias('{#', $.delimiter),
      field('name', $.section_name),
      repeat(field('argument', $.section_argument)),
      optional(field('self_closing', '/')),
      alias('}', $.delimiter),
    ),

    section_close: $ => seq(
      alias('{/', $.delimiter),
      optional(field('name', $.section_name)),
      alias('}', $.delimiter),
    ),

    section_argument: $ => choice(
      $.named_argument,
      $.template_reference,
      $.expression,
    ),

    named_argument: $ => seq(
      field('name', $.identifier),
      field('operator', '='),
      field('value', $.expression),
    ),

    template_reference: _ => token(/[A-Za-z0-9_][A-Za-z0-9_.-]*[\/$-][A-Za-z0-9_.$\/-]*/),

    expression: $ => choice(
      $.identifier,
      $.namespace_expression,
      $.null,
      $.boolean,
      $.string,
      $.integer,
      $.float,
      $.parenthesized_expression,
      $.unary_expression,
      $.binary_expression,
      $.member_expression,
      $.call_expression,
      $.index_expression,
      $.safe_expression,
    ),

    parenthesized_expression: $ => seq('(', $.expression, ')'),

    namespace_expression: $ => seq(
      field('namespace', $.identifier),
      ':',
      field('name', $.identifier),
    ),

    unary_expression: $ => prec.right(PREC.unary, seq(
      field('operator', choice('!', '-')),
      field('operand', $.expression),
    )),

    binary_expression: $ => choice(
      ...[
        [PREC.elvis, '?:'],
        [PREC.or, '||'],
        [PREC.and, choice('&&', 'and')],
        [PREC.equality, choice('==', '!=', 'eq', 'is', 'ne')],
        [PREC.comparison, choice('<', '<=', '>', '>=', 'lt', 'le', 'gt', 'ge')],
        [PREC.additive, choice('+', '-')],
        [PREC.multiplicative, choice('*', '/', '%')],
      ].map(([precedence, operator]) => prec.left(/** @type {number} */ (precedence), seq(
        field('left', $.expression),
        field('operator', operator),
        field('right', $.expression),
      ))),
    ),

    member_expression: $ => prec.left(PREC.postfix, seq(
      field('object', $.expression),
      '.',
      field('member', $.identifier),
    )),

    call_expression: $ => prec.left(PREC.postfix, seq(
      field('function', $.expression),
      '(',
      optional(seq($.expression, repeat(seq(',', $.expression)))),
      ')',
    )),

    index_expression: $ => prec.left(PREC.postfix, seq(
      field('object', $.expression),
      '[',
      field('index', $.expression),
      ']',
    )),

    safe_expression: $ => prec.left(PREC.postfix, seq(
      field('value', $.expression),
      field('operator', '??'),
    )),

    section_name: _ => /[A-Za-z_][A-Za-z0-9_-]*/,
    type_identifier: _ => /[^\s}=]+/,
    identifier: _ => /[A-Za-z_][\p{L}\p{N}_]*/u,
    null: _ => 'null',
    boolean: _ => choice('true', 'false'),
    string: _ => choice(
      token(seq('"', repeat(choice(/[^"\\]+/, /\\./)), optional('"'))),
      token(seq('\'', repeat(choice(/[^'\\]+/, /\\./)), optional('\''))),
    ),
    float: _ => /[0-9]+\.[0-9]+/,
    integer: _ => /[0-9]+/,
  },
});
