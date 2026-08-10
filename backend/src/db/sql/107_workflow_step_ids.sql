-- Give every persisted workflow step a durable UUID distinct from its
-- editable `name`. JSON arrays stay ordered and every existing non-empty id
-- is preserved, making the migration safe to re-run during development.

UPDATE workflows
SET steps_json = (
    SELECT json_group_array(json(step_json))
    FROM (
        SELECT json_set(
            value,
            '$.id',
            COALESCE(
                NULLIF(json_extract(value, '$.id'), ''),
                lower(hex(randomblob(4))) || '-' ||
                lower(hex(randomblob(2))) || '-4' ||
                substr(lower(hex(randomblob(2))), 2) || '-' ||
                substr('89ab', 1 + (abs(random()) % 4), 1) ||
                substr(lower(hex(randomblob(2))), 2) || '-' ||
                lower(hex(randomblob(6)))
            )
        ) AS step_json
        FROM json_each(workflows.steps_json)
        ORDER BY CAST(key AS INTEGER)
    )
)
WHERE json_valid(steps_json)
  AND json_type(steps_json) = 'array';

UPDATE workflows
SET on_failure = (
    SELECT json_group_array(json(step_json))
    FROM (
        SELECT json_set(
            value,
            '$.id',
            COALESCE(
                NULLIF(json_extract(value, '$.id'), ''),
                lower(hex(randomblob(4))) || '-' ||
                lower(hex(randomblob(2))) || '-4' ||
                substr(lower(hex(randomblob(2))), 2) || '-' ||
                substr('89ab', 1 + (abs(random()) % 4), 1) ||
                substr(lower(hex(randomblob(2))), 2) || '-' ||
                lower(hex(randomblob(6)))
            )
        ) AS step_json
        FROM json_each(workflows.on_failure)
        ORDER BY CAST(key AS INTEGER)
    )
)
WHERE on_failure IS NOT NULL
  AND json_valid(on_failure)
  AND json_type(on_failure) = 'array';
