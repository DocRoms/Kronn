-- KT-251 — the identity an in-flight answer will keep.
--
-- Until now the recovered fragment's id was minted at RECOVERY time, so the
-- answer a human watched being written had no id at all while it was being
-- written, and got a brand-new one afterwards. Nobody could name the thing that
-- was misbehaving while it misbehaved — reported verbatim: "je ne vois pas
-- encore d'id, faudrait qu'on l'ait direct, ça t'aurait aidé au debug".
--
-- Assigned with the first checkpoint and reused by the recovery, so the id is
-- stable from the first streamed token to the salvaged message. Cleared with
-- the rest of the checkpoint on normal completion.

ALTER TABLE discussions ADD COLUMN partial_response_message_id TEXT;
