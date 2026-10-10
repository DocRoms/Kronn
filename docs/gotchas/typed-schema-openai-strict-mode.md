# TypedSchema on the OpenAI wire (KT-717)

Related: [HTTP model-provider transport](../operations/http-transport.md).

## Official contract verification — 2026-09-27

OpenAI's [Structured Outputs guide](https://developers.openai.com/api/docs/guides/structured-outputs)
requires every object to set `additionalProperties: false` and every declared
property to appear in `required` when using strict mode. Optional values can
be represented with a nullable required property. Its exact documented failure
statement ends:

> call the API with an unsupported JSON Schema, you will receive an error.

This is documentation-based verification, not a live provider request. No HTTP
status or provider error payload was observed.

The workflow envelope defines `data`, `status`, and `summary`, requires only
`data` and `status`, and omits `additionalProperties`. It embeds the author's
schema unchanged. The existing regression fixture also contains an object
with an optional string field and no `additionalProperties` restriction.
[src: file: backend/src/workflows/steps.rs:947]
[src: file: backend/src/workflows/steps.rs:1895]

Inference from that code and the official contract: this envelope is invalid
under OpenAI strict mode, independently of whether a proxy accepts it.

## Decision

Send `response_format.type: "json_schema"` with
`response_format.json_schema.strict: false` on the OpenAI wire, preserving the
supplied schema. This is an explicit request setting, not an error-triggered
fallback. Requests without a schema omit `response_format`.
[src: file: backend/src/agents/chat_codec.rs:182]

Rationale: preserve optional-field omission and open objects instead of
normalizing arbitrary author schemas. Making an optional string required and
nullable would allow a null that the original schema rejects during local
validation. The validator checks present properties recursively and compares
their actual type to the authored string type.
[src: file: backend/src/workflows/template.rs:1200]
[src: file: backend/src/workflows/template.rs:1269]

Ollama still receives the same envelope through its native `format` field,
with a non-streaming request. No change to its decoding schema is required.
[src: file: backend/src/agents/runner.rs:7319]
[src: file: backend/src/workflows/steps.rs:947]

## Guarantees and regression coverage

OpenAI [documents both strict modes](https://developers.openai.com/api/docs/guides/prompt-generation),
but only strict mode guarantees exact schema adherence. Kronn still extracts
the envelope, validates `data`, attempts repair, and applies `on_invalid`.
That local validator supports a subset of JSON Schema; it is not a replacement
for full provider-enforced validation. For example, it does not enforce
`additionalProperties` or resolve `$ref`.
[src: file: backend/src/workflows/steps.rs:427]
[src: file: backend/src/workflows/steps.rs:692]
[src: file: backend/src/workflows/template.rs:1200]

Body regressions cover explicit `strict: false`, preservation of nested optional
properties, array items and dictionary schemas, both streaming settings, and
absence of `response_format` when no schema is supplied. The workflow regression
passes the actual envelope to both request builders and checks the unchanged
Ollama `format` and the non-strict OpenAI schema.
[src: file: backend/src/agents/chat_codec.rs:507]
[src: file: backend/src/workflows/steps.rs:1895]

Schema errors still do not trigger the KT-709 unsupported-feature fallback;
the existing refusal regressions exercise that distinction.
[src: file: backend/src/agents/runner_test.rs:3993]
[src: file: backend/src/agents/runner_test.rs:4147]
