use audit_database::AuditDatabaseContext;
use audit_logs_stream::AuditLogsStream;
use color_eyre::eyre::OptionExt;
use dal::{prop::PropPath, AttributeValue, DalContext, Prop, Schema, SchemaVariant};
use dal_test::helpers::{
    confirm_jetstream_stream_has_no_messages,
    create_named_component_for_schema_variant_on_default_view,
    list_audit_logs_until_expected_number_of_rows,
};
use dal_test::{helpers::ChangeSetTestHelpers, test};
use pending_events::PendingEventsStream;
use pretty_assertions_sorted::assert_eq;
use si_events::audit_log::AuditLogKind;

const DATABASE_RETRY_TIMEOUT_SECONDS: u64 = 2;
const DATABASE_RETRY_INTERVAL_MILLISECONDS: u64 = 100;

const STREAM_RETRY_TIMEOUT_SECONDS: u64 = 5;
const STREAM_RETRY_INTERVAL_MILLISECONDS: u64 = 100;

const SIZE: usize = 200;

#[test]
async fn round_trip(
    ctx: &mut DalContext,
    audit_database_context: AuditDatabaseContext,
) -> color_eyre::Result<()> {
    let context = audit_database_context;

    // Collect schema information.
    let schema = Schema::find_by_name(ctx, "swifty")
        .await?
        .ok_or_eyre("schema not found by name")?;
    let schema_variant_id = schema
        .get_default_schema_variant_id(ctx)
        .await?
        .ok_or_eyre("no default schema variant id found")?;
    let schema_variant = SchemaVariant::get_by_id_or_error(ctx, schema_variant_id).await?;

    // Create a component and commit. Mimic sdf by audit logging here.
    let component_name = "nyj despair_club";
    let component = create_named_component_for_schema_variant_on_default_view(
        ctx,
        component_name,
        schema_variant_id,
    )
    .await?;
    ctx.write_audit_log(
        AuditLogKind::CreateComponent {
            name: component_name.to_string(),
            component_id: component.id(),
            schema_variant_id,
            schema_variant_name: schema_variant.display_name().to_string(),
        },
        component_name.to_string(),
    )
    .await?;
    ChangeSetTestHelpers::commit_and_update_snapshot_to_visibility(ctx).await?;

    // Collect the streams needed throughout the test.
    let (source_stream, destination_stream) = {
        let source_stream_wrapper =
            PendingEventsStream::get_or_create(ctx.jetstream_context()).await?;
        let destination_stream_wrapper =
            AuditLogsStream::get_or_create(ctx.jetstream_context()).await?;
        let source_stream = source_stream_wrapper.stream().await?;
        let destination_stream = destination_stream_wrapper.stream().await?;
        (source_stream, destination_stream)
    };

    // Check that everything looks as we expect.
    {
        let expected_total = 5;
        confirm_jetstream_stream_has_no_messages(
            &source_stream,
            STREAM_RETRY_TIMEOUT_SECONDS,
            STREAM_RETRY_INTERVAL_MILLISECONDS,
        )
        .await?;
        let destination_stream_message_count = destination_stream.get_info().await?.state.messages;
        assert_eq!(
            expected_total,                   // expected
            destination_stream_message_count  // actual
        );
        list_audit_logs_until_expected_number_of_rows(
            ctx,
            &context,
            SIZE,
            expected_total as usize,
            DATABASE_RETRY_TIMEOUT_SECONDS,
            DATABASE_RETRY_INTERVAL_MILLISECONDS,
        )
        .await?;
    }

    // Update a property editor value and commit. Mimic sdf by audit logging here.
    let prop_path_raw = ["root", "domain", "name"];
    let prop =
        Prop::find_prop_by_path(ctx, schema_variant_id, &PropPath::new(prop_path_raw)).await?;
    let mut attribute_value_ids = component
        .attribute_values_for_prop(ctx, &prop_path_raw)
        .await?;
    let attribute_value_id = attribute_value_ids
        .pop()
        .ok_or_eyre("no attribute values found")?;
    assert!(attribute_value_ids.is_empty());
    let before_value = AttributeValue::get_by_id(ctx, attribute_value_id)
        .await?
        .value(ctx)
        .await?;
    let after_value = Some(serde_json::json!("pain."));
    AttributeValue::update(ctx, attribute_value_id, after_value.to_owned()).await?;
    ctx.write_audit_log(
        AuditLogKind::UpdatePropertyEditorValue {
            component_id: component.id(),
            component_name: component_name.to_string(),
            schema_variant_id,
            schema_variant_display_name: schema_variant.display_name().to_string(),
            prop_id: prop.id,
            prop_name: prop.name.to_owned(),
            attribute_value_id,
            before_value,
            after_value,
        },
        component_name.to_string(),
    )
    .await?;
    ChangeSetTestHelpers::commit_and_update_snapshot_to_visibility(ctx).await?;

    // Check that everything looks as we expect.
    {
        let expected_total = 9;
        confirm_jetstream_stream_has_no_messages(
            &source_stream,
            STREAM_RETRY_TIMEOUT_SECONDS,
            STREAM_RETRY_INTERVAL_MILLISECONDS,
        )
        .await?;
        let destination_stream_message_count = destination_stream.get_info().await?.state.messages;
        assert_eq!(
            expected_total,                   // expected
            destination_stream_message_count  // actual
        );
        list_audit_logs_until_expected_number_of_rows(
            ctx,
            &context,
            SIZE,
            expected_total as usize,
            DATABASE_RETRY_TIMEOUT_SECONDS,
            DATABASE_RETRY_INTERVAL_MILLISECONDS,
        )
        .await?;
    }

    // Delete a component and commit. Mimic sdf by audit logging here.
    ctx.write_audit_log(
        AuditLogKind::DeleteComponent {
            name: component_name.to_string(),
            component_id: component.id(),
            schema_variant_id,
            schema_variant_name: schema_variant.display_name().to_string(),
        },
        component_name.to_string(),
    )
    .await?;
    assert!(component.delete(ctx).await?.is_none());
    ChangeSetTestHelpers::commit_and_update_snapshot_to_visibility(ctx).await?;

    // Check that everything looks as we expect.
    {
        let expected_total = 10;
        confirm_jetstream_stream_has_no_messages(
            &source_stream,
            STREAM_RETRY_TIMEOUT_SECONDS,
            STREAM_RETRY_INTERVAL_MILLISECONDS,
        )
        .await?;
        let destination_stream_message_count = destination_stream.get_info().await?.state.messages;
        assert_eq!(
            expected_total,                   // expected
            destination_stream_message_count  // actual
        );
        list_audit_logs_until_expected_number_of_rows(
            ctx,
            &context,
            SIZE,
            expected_total as usize,
            DATABASE_RETRY_TIMEOUT_SECONDS,
            DATABASE_RETRY_INTERVAL_MILLISECONDS,
        )
        .await?;
    }

    Ok(())
}
