-- WAL-G PITR Configuration
-- Add S3/credentials columns to db_pitr_config for WAL-G based PITR

ALTER TABLE db_pitr_config
    ADD COLUMN IF NOT EXISTS s3_bucket TEXT,
    ADD COLUMN IF NOT EXISTS s3_region TEXT,
    ADD COLUMN IF NOT EXISTS s3_endpoint TEXT,
    ADD COLUMN IF NOT EXISTS aws_access_key_id TEXT,
    ADD COLUMN IF NOT EXISTS aws_secret_access_key TEXT; -- encrypted

CREATE INDEX IF NOT EXISTS idx_db_pitr_config_enabled ON db_pitr_config(pitr_enabled) WHERE pitr_enabled = true;