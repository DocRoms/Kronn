-- Whether the Kronn tool executor is attached to a run on this external API
-- connection (KT-1600). NULL means "not declared": a connection whose proxy
-- cannot report model capabilities (LiteLLM, a custom gateway) then gets no
-- tools, rather than a tool schema its model may reject.
ALTER TABLE external_api_connections ADD COLUMN tools_enabled INTEGER;
