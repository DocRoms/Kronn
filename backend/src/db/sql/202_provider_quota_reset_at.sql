-- KT-838: when the provider's own refusal announces a reset time ("resets
-- 4:20pm (Europe/Paris)"), keep it beside the provider's quota generation so
-- Settings > Agents can say "rearmable at 16:20". It is only ever displayed:
-- nothing re-arms a provider from this column (KT-593).
ALTER TABLE provider_quota_generations ADD COLUMN reset_at TEXT;
