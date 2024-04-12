use dal::{
    attribute::context::{AttributeContext, AttributeContextBuilder},
    attribute::prototype::AttributePrototype,
    func::{backend::string::FuncBackendStringArgs, binding::FuncBinding},
    AttributePrototypeError, AttributeValue, Component, ComponentView, DalContext, Func,
    FuncBackendKind, FuncBackendResponseType, PropKind, Schema, SchemaVariant, StandardModel,
};
use dal_test::helpers::component_bag::ComponentBagger;
use dal_test::{
    test,
    test_harness::{create_component_for_schema, create_schema, create_schema_variant_with_root},
};
use pretty_assertions_sorted::assert_eq;

#[test]
async fn new_attribute_prototype(ctx: &DalContext) {
    let mut bagger = ComponentBagger::new();
    let component_bag = bagger.create_component(ctx, "poop", "starfield").await;
    let schema_variant = component_bag.schema_variant(ctx).await;

    let func = Func::new(
        ctx,
        "test:setString",
        FuncBackendKind::String,
        FuncBackendResponseType::String,
    )
    .await
    .expect("cannot create func");

    let args = FuncBackendStringArgs::new("eldenring".to_string());

    let (func_binding, func_binding_return_value) = FuncBinding::create_and_execute(
        ctx,
        serde_json::to_value(args).expect("cannot turn args into json"),
        *func.id(),
        vec![],
    )
    .await
    .expect("failed to execute func binding");

    let root_prop_id = schema_variant
        .root_prop_id()
        .expect("no root prop for schema variant");
    let context = AttributeContext::builder()
        .set_prop_id(*root_prop_id)
        .set_component_id(component_bag.component_id)
        .to_context()
        .expect("cannot create context");
    let _attribute_prototype = AttributePrototype::new(
        ctx,
        *func.id(),
        *func_binding.id(),
        *func_binding_return_value.id(),
        context,
        None,
        None,
    )
    .await
    .expect("cannot create new attribute prototype");
}

#[test]
async fn list_for_context_with_a_hash(ctx: &DalContext) {
    let mut schema = create_schema(ctx).await;
    let (mut schema_variant, root) = create_schema_variant_with_root(ctx, *schema.id()).await;
    schema
        .set_default_schema_variant_id(ctx, Some(*schema_variant.id()))
        .await
        .expect("cannot set default schema variant");
    let base_prototype_context = AttributeContext::builder();
    let schema_variant_id = *schema_variant.id();

    // {
    //   albums: [
    //     { String: String, },
    //   ]
    // }
    let albums_prop = dal_test::test_harness::create_prop_without_ui_optionals(
        ctx,
        "albums_array",
        PropKind::Array,
        schema_variant_id,
        Some(root.domain_prop_id),
    )
    .await;
    let album_prop = dal_test::test_harness::create_prop_without_ui_optionals(
        ctx,
        "album_object",
        PropKind::Map,
        schema_variant_id,
        Some(*albums_prop.id()),
    )
    .await;
    let hash_key_prop = dal_test::test_harness::create_prop_without_ui_optionals(
        ctx,
        "album_hash_key",
        PropKind::String,
        schema_variant_id,
        Some(*album_prop.id()),
    )
    .await;
    schema_variant
        .finalize(ctx, None)
        .await
        .expect("could not finalize schema variant");

    let domain_context = base_prototype_context
        .clone()
        .set_prop_id(root.domain_prop_id)
        .to_context()
        .expect("cannot build domain attribute context");

    let domain_attribute_value = AttributeValue::find_for_context(ctx, domain_context.into())
        .await
        .expect("cannot retrieve domain AttributeValue")
        .expect("cannot find domain AttributeValue");

    let albums_prototype_context = base_prototype_context
        .clone()
        .set_prop_id(*albums_prop.id())
        .to_context()
        .expect("cannot build attribute context");

    let album_prototype_context = base_prototype_context
        .clone()
        .set_prop_id(*album_prop.id())
        .to_context()
        .expect("cannot build attribute context");

    let prop_hash_key_prototype_context = base_prototype_context
        .clone()
        .set_prop_id(*hash_key_prop.id())
        .to_context()
        .expect("cannot build attribute context");

    let albums_attribute_value =
        AttributeValue::find_for_context(ctx, albums_prototype_context.into())
            .await
            .expect("cannot retrieve albums AttributeValue")
            .expect("albums AttribtueValue not found");

    let (_, albums_attribute_value_id) = AttributeValue::update_for_context(
        ctx,
        *albums_attribute_value.id(),
        Some(*domain_attribute_value.id()),
        albums_prototype_context,
        Some(serde_json::json!([])),
        None,
    )
    .await
    .expect("cannot update albums AttributeValue");

    let undertow_hash_attribute_value_id = AttributeValue::insert_for_context(
        ctx,
        albums_prototype_context,
        albums_attribute_value_id,
        Some(serde_json::json!({})),
        None,
    )
    .await
    .expect("cannot create hash for Undertow");
    let undertow_attribute_value_id = AttributeValue::insert_for_context(
        ctx,
        album_prototype_context,
        undertow_hash_attribute_value_id,
        Some(serde_json::json!("1993")),
        Some("Undertow".to_string()),
    )
    .await
    .expect("cannot create Undertow entry");
    let undertow_prop_prototype = AttributeValue::get_by_id(ctx, &undertow_attribute_value_id)
        .await
        .expect("cannot retrieve AttributeValue")
        .expect("cannot find AttributeValue")
        .attribute_prototype(ctx)
        .await
        .expect("cannot retrieve AttributePrototype")
        .expect("cannot find AttributePrototype");

    let albums_attribute_value_id =
        *AttributeValue::find_for_context(ctx, albums_prototype_context.into())
            .await
            .expect("cannot retrieve AttributeValue")
            .expect("cannot find AttributeValue")
            .id();

    let lateralus_hash_attribute_value_id = AttributeValue::insert_for_context(
        ctx,
        albums_prototype_context,
        albums_attribute_value_id,
        Some(serde_json::json!({})),
        None,
    )
    .await
    .expect("cannot create hash for Lateralus");
    let lateralus_attribute_value_id = AttributeValue::insert_for_context(
        ctx,
        album_prototype_context,
        lateralus_hash_attribute_value_id,
        Some(serde_json::json!("2001")),
        Some("Lateralus".to_string()),
    )
    .await
    .expect("cannot create Lateralus entry");
    let lateralus_prop_prototype = AttributeValue::get_by_id(ctx, &lateralus_attribute_value_id)
        .await
        .expect("cannot retrieve AttributeValue")
        .expect("cannot find AttributeValue")
        .attribute_prototype(ctx)
        .await
        .expect("cannot retrieve AttributePrototype")
        .expect("cannot retrieve AttributePrototype");

    let component = create_component_for_schema(ctx, schema.id()).await;

    let component_album_prototype_context = AttributeContextBuilder::from(album_prototype_context)
        .clone()
        .set_component_id(*component.id())
        .to_context()
        .expect("cannot create component array entry AttributeContext");
    let component_hash_key_prototype_context =
        AttributeContextBuilder::from(prop_hash_key_prototype_context)
            .clone()
            .set_component_id(*component.id())
            .to_context()
            .expect("cannot create component hash AttributeContext");

    let albums_component_context = AttributeContextBuilder::from(albums_prototype_context)
        .set_component_id(*component.id())
        .to_context()
        .expect("cannot create albums component AttributeContext");

    let (_, lateralus_component_attribute_value_id) = AttributeValue::update_for_context(
        ctx,
        lateralus_attribute_value_id,
        Some(lateralus_hash_attribute_value_id),
        component_hash_key_prototype_context,
        Some(serde_json::json!("The Early 2000s")),
        Some("Lateralus".to_string()),
    )
    .await
    .expect("cannot set Lateralus entry for component");

    let albums_attribute_value_id =
        *AttributeValue::find_for_context(ctx, albums_component_context.into())
            .await
            .expect("cannot retrieve AttributeValue")
            .expect("cannot find AttributeValue")
            .id();

    let lateralus_component_prototype =
        AttributeValue::get_by_id(ctx, &lateralus_component_attribute_value_id)
            .await
            .expect("cannot retrieve AttributeValue")
            .expect("cannot find AttributeValue")
            .attribute_prototype(ctx)
            .await
            .expect("cannot retrieve AttributePrototype")
            .expect("cannot find AttributePrototype");

    let fear_inoculum_hash_attribute_value_id = AttributeValue::insert_for_context(
        ctx,
        albums_component_context,
        albums_attribute_value_id,
        Some(serde_json::json!({})),
        None,
    )
    .await
    .expect("cannot create Fear Inoculum array entry");
    let fear_inoculum_attribute_value_id = AttributeValue::insert_for_context(
        ctx,
        component_album_prototype_context,
        fear_inoculum_hash_attribute_value_id,
        Some(serde_json::json!("2019")),
        Some("Fear Inoculum".to_string()),
    )
    .await
    .expect("cannot set Fear Inoculum entry for component");
    let fear_inoculum_component_prototype =
        AttributeValue::get_by_id(ctx, &fear_inoculum_attribute_value_id)
            .await
            .expect("cannot retrieve AttributeValue")
            .expect("cannot find AttributeValue")
            .attribute_prototype(ctx)
            .await
            .expect("cannot retrieve AttributePrototype")
            .expect("cannot find AttributePrototype");

    let found_hash_key_prototypes =
        AttributePrototype::list_for_context(ctx, component_hash_key_prototype_context)
            .await
            .expect("could not retrieve component prototypes");

    let mut hash_key_values = vec![];
    for proto in found_hash_key_prototypes.clone() {
        hash_key_values.extend(
            proto
                .attribute_values(ctx)
                .await
                .expect("could not retrieve values for prototype"),
        );
    }

    assert_eq!(
        vec![
            fear_inoculum_component_prototype,
            lateralus_component_prototype,
            undertow_prop_prototype.clone(),
        ],
        found_hash_key_prototypes,
    );

    let found_hash_key_prototypes =
        AttributePrototype::list_for_context(ctx, prop_hash_key_prototype_context)
            .await
            .expect("could not retrieve prop prototypes");

    assert_eq!(
        vec![lateralus_prop_prototype, undertow_prop_prototype],
        found_hash_key_prototypes,
    );
}

/// Test attribute prototype removal corresponding to a least specific context.
#[test]
async fn remove_least_specific(ctx: &DalContext) {
    let schema = Schema::find_by_name(ctx, "starfield")
        .await
        .expect("could not find schema");
    let schema_variant_id = *schema
        .default_schema_variant_id()
        .expect("could not find default schema variant id");
    let domain_prop = SchemaVariant::find_prop_in_tree(ctx, schema_variant_id, &["root", "domain"])
        .await
        .expect("could not find prop");
    let prop = dal_test::test_harness::create_prop_without_ui_optionals(
        ctx,
        "toddhoward",
        PropKind::String,
        schema_variant_id,
        Some(*domain_prop.id()),
    )
    .await;
    let context = AttributeContextBuilder::new()
        .set_prop_id(*prop.id())
        .to_context()
        .expect("could not build context");

    let prototypes = AttributePrototype::list_for_context(ctx, context)
        .await
        .expect("could not list attribute prototypes for context");

    for prototype in prototypes {
        let result = AttributePrototype::remove(ctx, prototype.id(), false).await;
        if let Err(AttributePrototypeError::LeastSpecificContextPrototypeRemovalNotAllowed(id)) =
            result
        {
            assert_eq!(prototype.id(), &id);
        } else {
            panic!("expected least-specific context not allowed for removal error, found the following result: {result:?}");
        }
    }
}

/// Test attribute prototype removal corresponding to a component-specific context.
#[test]
async fn remove_component_specific(ctx: &DalContext) {
    let mut schema = create_schema(ctx).await;
    let (mut schema_variant, root) = create_schema_variant_with_root(ctx, *schema.id()).await;
    schema
        .set_default_schema_variant_id(ctx, Some(*schema_variant.id()))
        .await
        .expect("cannot set default schema variant");

    let prop = dal_test::test_harness::create_prop_without_ui_optionals(
        ctx,
        "god",
        PropKind::String,
        *schema_variant.id(),
        Some(root.domain_prop_id),
    )
    .await;
    schema_variant
        .finalize(ctx, None)
        .await
        .expect("cannot finalize SchemaVariant");

    ctx.blocking_commit()
        .await
        .expect("could not commit & run jobs");

    let (component, _) =
        Component::new_for_default_variant_from_schema(ctx, "toddhoward", *schema.id())
            .await
            .expect("cannot create component");

    ctx.blocking_commit()
        .await
        .expect("could not commit & run jobs");

    let component_view = ComponentView::new(ctx, *component.id())
        .await
        .expect("cannot get component view");

    assert_eq!(
        serde_json::json![
            {
                "si": {
                    "name": "toddhoward",
                    "type": "component",
                    "protected": false
                },
                "domain": {},
                "resource": {},
            }
        ],
        component_view.properties,
    );

    let context = AttributeContextBuilder::new()
        .set_prop_id(*prop.id())
        .set_component_id(*component.id())
        .to_context()
        .expect("could not build context");

    let prototypes = AttributePrototype::list_for_context(ctx, context)
        .await
        .expect("could not list attribute prototypes for context");

    for prototype in prototypes {
        // Ensure that performing remove on base prototypes on props results in failure.
        assert!(AttributePrototype::remove(ctx, prototype.id(), false)
            .await
            .is_err());

        // Update the prototype for our component-specific context using its immediate value(s).
        // Updating each value for our context will result in our prototype being updated as well.
        let values = prototype
            .attribute_values(ctx)
            .await
            .expect("could not get attribute values");
        for value in values {
            let parent_value_id = value
                .parent_attribute_value(ctx)
                .await
                .expect("could not get parent attribute_value")
                .map(|parent| *parent.id());

            let _ = AttributeValue::update_for_context(
                ctx,
                *value.id(),
                parent_value_id,
                context,
                None,
                None,
            )
            .await
            .expect("could not update value");
        }

        ctx.blocking_commit()
            .await
            .expect("could not commit & run jobs");

        // Now that the prototype's value(s) have been updated with our component-specific context,
        // we can perform removal.
        let updated_prototypes = AttributePrototype::list_for_context(ctx, context)
            .await
            .expect("could not list attribute prototypes for context");

        for updated_prototype in updated_prototypes {
            // Find all the nested values and their corresponding prototypes for the updated
            // prototype. We will need them to check if they have been successfully deleted.
            let updated_values = updated_prototype
                .attribute_values(ctx)
                .await
                .expect("could not get attribute values");

            let mut confirm_deletion_prototype_ids = vec![*updated_prototype.id()];
            let mut confirm_deletion_value_ids = Vec::new();

            let mut nested_values_work_queue = updated_values;
            while let Some(nested_value) = nested_values_work_queue.pop() {
                let child_attribute_values = nested_value
                    .child_attribute_values(ctx)
                    .await
                    .expect("could not get child attribute values");
                if !child_attribute_values.is_empty() {
                    nested_values_work_queue.extend(child_attribute_values);
                }
                if let Some(current_prototype) = nested_value
                    .attribute_prototype(ctx)
                    .await
                    .expect("could not get attribute prototype")
                {
                    confirm_deletion_prototype_ids.push(*current_prototype.id());
                }
                confirm_deletion_value_ids.push(*nested_value.id());
            }

            // Perform removal on the prototype.
            assert!(
                AttributePrototype::remove(ctx, updated_prototype.id(), false)
                    .await
                    .is_ok()
            );

            ctx.blocking_commit()
                .await
                .expect("could not commit & run jobs");

            // Confirm the prototype, its nested values and their corresponding prototypes have
            // been deleted.
            for confirm_deletion_prototype_id in &confirm_deletion_prototype_ids {
                assert!(
                    AttributePrototype::get_by_id(ctx, confirm_deletion_prototype_id)
                        .await
                        .is_err()
                );
            }
            for confirm_deletion_value_id in confirm_deletion_value_ids {
                assert!(AttributeValue::get_by_id(ctx, &confirm_deletion_value_id)
                    .await
                    .expect("could not get attribute value by id")
                    .is_none());
            }
        }
    }
}
