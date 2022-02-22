CREATE TABLE encrypted_secrets
(
    pk                          bigserial PRIMARY KEY,
    id                          bigserial                NOT NULL,
    tenancy_universal           bool                     NOT NULL,
    tenancy_billing_account_ids bigint[],
    tenancy_organization_ids    bigint[],
    tenancy_workspace_ids       bigint[],
    visibility_change_set_pk    bigint                   NOT NULL DEFAULT -1,
    visibility_edit_session_pk  bigint                   NOT NULL DEFAULT -1,
    visibility_deleted          bool,
    created_at                  timestamp with time zone NOT NULL DEFAULT NOW(),
    updated_at                  timestamp with time zone NOT NULL DEFAULT NOW(),
    name                        text                     NOT NULL,
    object_type                 text                     NOT NULL,
    kind                        text                     NOT NULL,
    billing_account_id          bigint                   NOT NULL,
    crypted                     text                     NOT NULL,
    version                     text                     NOT NULL,
    algorithm                   text                     NOT NULL
);
SELECT standard_model_table_constraints_v1('encrypted_secrets');
SELECT belongs_to_table_create_v1('encrypted_secret_belongs_to_workspace', 'encrypted_secrets', 'workspaces');
SELECT belongs_to_table_create_v1('encrypted_secret_belongs_to_key_pair', 'encrypted_secrets', 'key_pairs');

INSERT INTO standard_models (table_name, table_type, history_event_label_base, history_event_message_name)
VALUES ('encrypted_secrets', 'model', 'encrypted_secret', 'Encrypted Secret'),
       ('encrypted_secret_belongs_to_workspace', 'belongs_to', 'encrypted_secret.workspace',
        'Encrypted Secret <> Workspace'),
       ('encrypted_secret_belongs_to_key_pair', 'belongs_to', 'encrypted_secret.key_pair',
        'Encrypted Secret <> Key Pair');

-- The Rust type `Secret` will use this view as its source-of-truth "table" as
-- it is a read-only subset of encrypted_secrets data
CREATE VIEW secrets AS
SELECT pk,
       id,
       tenancy_universal,
       tenancy_billing_account_ids,
       tenancy_organization_ids,
       tenancy_workspace_ids,
       visibility_change_set_pk,
       visibility_edit_session_pk,
       visibility_deleted,
       created_at,
       updated_at,
       name,
       object_type,
       kind
FROM encrypted_secrets;

CREATE OR REPLACE FUNCTION encrypted_secret_create_v1(
    this_tenancy jsonb,
    this_visibility jsonb,
    this_name text,
    this_object_type text,
    this_kind text,
    this_crypted text,
    this_version text,
    this_algorithm text,
    this_billing_account_id bigint,
    OUT object json) AS
$$
DECLARE
    this_tenancy_record    tenancy_record_v1;
    this_visibility_record visibility_record_v1;
    this_new_row           encrypted_secrets%ROWTYPE;
BEGIN
    this_tenancy_record := tenancy_json_to_columns_v1(this_tenancy);
    this_visibility_record := visibility_json_to_columns_v1(this_visibility);

    INSERT INTO encrypted_secrets (tenancy_universal,
                                   tenancy_billing_account_ids,
                                   tenancy_organization_ids,
                                   tenancy_workspace_ids,
                                   visibility_change_set_pk,
                                   visibility_edit_session_pk,
                                   visibility_deleted,
                                   name,
                                   object_type,
                                   kind,
                                   billing_account_id,
                                   crypted,
                                   version,
                                   algorithm)
    VALUES (this_tenancy_record.tenancy_universal,
            this_tenancy_record.tenancy_billing_account_ids,
            this_tenancy_record.tenancy_organization_ids,
            this_tenancy_record.tenancy_workspace_ids,
            this_visibility_record.visibility_change_set_pk,
            this_visibility_record.visibility_edit_session_pk,
            this_visibility_record.visibility_deleted,
            this_name,
            this_object_type,
            this_kind,
            this_billing_account_id,
            this_crypted,
            this_version,
            this_algorithm)
    RETURNING * INTO this_new_row;

    -- Purge the returning record of sensitive data to avoid accidentally
    -- deserializing these fields in application code
    this_new_row.crypted = null;
    this_new_row.version = null;
    this_new_row.algorithm = null;

    object := row_to_json(this_new_row);
END;
$$ LANGUAGE PLPGSQL VOLATILE;
