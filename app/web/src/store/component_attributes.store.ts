import { defineStore } from "pinia";
import * as _ from "lodash-es";
import { addStoreHooks, ApiRequest } from "@si/vue-lib/pinia";

import { useWorkspacesStore } from "@/store/workspaces.store";
import {
  PropertyEditorProp,
  PropertyEditorSchema,
  PropertyEditorValue,
  PropertyEditorValues,
  ValidationOutput,
} from "@/api/sdf/dal/property_editor";
import { useChangeSetsStore } from "./change_sets.store";
import { useRealtimeStore } from "./realtime/realtime.store";
import { ComponentId, useComponentsStore } from "./components.store";

export interface UpdatePropertyEditorValueArgs {
  attributeValueId: string;
  parentAttributeValueId?: string;
  propId: string;
  componentId: string;
  value?: unknown;
  key?: string;
  isForSecret: boolean;
}

export interface InsertPropertyEditorValueArgs {
  parentAttributeValueId: string;
  propId: string;
  componentId: string;
  value?: unknown;
  key?: string;
}

export interface DeletePropertyEditorValueArgs {
  attributeValueId: string;
  propId: string;
  componentId: string;
  value?: unknown;
  key?: string;
}

export interface ResetPropertyEditorValueArgs {
  attributeValueId: string;
}

export interface SetTypeArgs {
  componentId: string;
  value?: unknown;
}

export interface OutputStream {
  stream: string;
  level: string;
  group: string | null;
  message: string;
}

export type AttributeTreeItem = {
  propDef: PropertyEditorProp;
  children: AttributeTreeItem[];
  value: PropertyEditorValue | undefined;
  valueId: string;
  parentValueId: string;
  validation: ValidationOutput | null;
  propId: string;
  mapKey?: string;
  arrayKey?: string;
  arrayIndex?: number;
};

export const useComponentAttributesStore = (componentId: ComponentId) => {
  const changeSetsStore = useChangeSetsStore();
  const changeSetId = changeSetsStore.selectedChangeSetId;

  const visibilityParams = {
    visibility_change_set_pk: changeSetId,
  };
  const workspacesStore = useWorkspacesStore();
  const workspaceId = workspacesStore.selectedWorkspacePk;

  return addStoreHooks(
    defineStore(
      `ws${
        workspaceId || "NONE"
      }/cs${changeSetId}/c${componentId}/component_attributes`,
      {
        state: () => ({
          // TODO: likely want to restructure how this data is sent and stored
          // but we'll just move into a pinia store as the first step...
          schema: null as PropertyEditorSchema | null,
          values: null as PropertyEditorValues | null,
          batchedProps: [] as (
            | UpdatePropertyEditorValueArgs
            | InsertPropertyEditorValueArgs
          )[],
        }),
        getters: {
          // recombine the schema + values + validations into a single nested tree that can be used by the attributes panel
          attributesTree: (state): AttributeTreeItem | undefined => {
            if (!state.schema || !state.values) return;

            const valuesByValueId = state.values.values;
            const propsByPropId = state.schema.props;
            const rootValueId = state.values.rootValueId;

            if (!valuesByValueId || !propsByPropId || !rootValueId) return;

            function getAttributeValueWithChildren(
              valueId: string,
              parentValueId: string,
              ancestorManual = true,
              indexInParentArray?: number,
            ): AttributeTreeItem | undefined {
              /* eslint-disable @typescript-eslint/no-non-null-assertion,@typescript-eslint/no-explicit-any */
              const value = valuesByValueId![valueId]!;

              const propDef = propsByPropId![value.propId as any];
              const validation = value?.validation ?? null;

              // some values that we see are for props that are hidden, so we filter them out
              if (!propDef) return;

              // console.log("HERE", value);

              value.ancestorManual = ancestorManual;
              const isAncestorManual =
                ancestorManual &&
                !value.isControlledByDynamicFunc &&
                !(value.canBeSetBySocket || value.isFromExternalSource);

              return {
                propDef,
                value,
                valueId,
                parentValueId,
                validation,
                // using isNil because its actually null (not undefined)
                ...(indexInParentArray === undefined &&
                  !_.isNil(value.key) && { mapKey: value.key }),
                ...(indexInParentArray !== undefined && {
                  arrayIndex: indexInParentArray,
                  arrayKey: value.key,
                }),
                propId: value.propId,
                children: _.compact(
                  _.map(state.values?.childValues[valueId], (cvId, index) =>
                    getAttributeValueWithChildren(
                      cvId,
                      valueId,
                      isAncestorManual,
                      propDef.kind === "array" ? index : undefined,
                    ),
                  ),
                ),
              };
            }

            // dummy parent root value id - not used by anything
            return getAttributeValueWithChildren(rootValueId, "ROOT");
          },
          domainTree(): AttributeTreeItem | undefined {
            if (!this.attributesTree) return undefined;
            return _.find(
              this.attributesTree.children,
              (c) => c.propDef.name === "domain",
            );
          },
          secretsTree(): AttributeTreeItem | undefined {
            if (!this.attributesTree) return undefined;
            return _.find(
              this.attributesTree.children,
              (c) => c.propDef.name === "secrets",
            );
          },
          siTreeByPropName(): Record<string, AttributeTreeItem> | undefined {
            if (!this.attributesTree) return undefined;
            const siTree = _.find(
              this.attributesTree.children,
              (c) => c.propDef.name === "si",
            );
            return _.keyBy(siTree?.children, (prop) => prop.propDef.name);
          },

          // getter to be able to quickly grab selected component id
          selectedComponentId: () => componentId,
          selectedComponent: () => {
            if (!componentId) return;
            const componentsStore = useComponentsStore();
            return componentsStore.componentsById[componentId];
          },
        },
        actions: {
          async FETCH_PROPERTY_EDITOR_SCHEMA() {
            return new ApiRequest<PropertyEditorSchema>({
              url: "component/get_property_editor_schema",
              params: {
                componentId: this.selectedComponentId,
                ...visibilityParams,
              },
              onSuccess: (response) => {
                if (this.selectedComponent === undefined) {
                  this.schema = response;
                  return;
                }

                const props: { [id: string]: PropertyEditorProp } = {};

                for (const propKey in response.props) {
                  const prop = response.props[propKey];
                  if (prop) {
                    const isHidden =
                      prop.name === "type" &&
                      this.selectedComponent.schemaName === "Generic Frame";
                    const isReadonly =
                      prop.name === "type" &&
                      this.selectedComponent.childIds !== undefined &&
                      this.selectedComponent.childIds.length > 0;

                    props[propKey] = {
                      ...prop,
                      isHidden,
                      isReadonly,
                    };
                  }
                }

                this.schema = { ...response, props };
              },
            });
          },
          async FETCH_PROPERTY_EDITOR_VALUES() {
            return new ApiRequest<PropertyEditorValues>({
              url: "component/get_property_editor_values",
              params: {
                componentId: this.selectedComponentId,
                ...visibilityParams,
              },
              onSuccess: (response) => {
                this.values = response;
              },
            });
          },

          reloadPropertyEditorData() {
            this.FETCH_PROPERTY_EDITOR_SCHEMA();
            this.FETCH_PROPERTY_EDITOR_VALUES();
          },

          async REMOVE_PROPERTY_VALUE(
            removePayload: DeletePropertyEditorValueArgs,
          ) {
            if (changeSetsStore.creatingChangeSet)
              throw new Error("race, wait until the change set is created");
            if (changeSetId === changeSetsStore.headChangeSetId)
              changeSetsStore.creatingChangeSet = true;

            return new ApiRequest<{ success: true }>({
              method: "post",
              url: "component/delete_property_editor_value",
              params: {
                ...removePayload,
                ...visibilityParams,
              },
            });
          },

          // NOTE this is async because we're returning the other
          // action that returns the APIRequest...
          async addPropertyToBatch(
            updatePayload:
              | { update: UpdatePropertyEditorValueArgs }
              | { insert: InsertPropertyEditorValueArgs },
          ) {
            let pushed = false;
            if ("insert" in updatePayload) {
              if (
                // don't allow duplicates
                !_.some(this.batchedProps, (b) =>
                  _.isEqual(updatePayload.insert, b),
                )
              ) {
                this.batchedProps.push(updatePayload.insert);
                pushed = true;
              }
            }
            if ("update" in updatePayload) {
              if (
                !_.some(this.batchedProps, (b) =>
                  _.isEqual(updatePayload.update, b),
                )
              ) {
                this.batchedProps.push(updatePayload.update);
                pushed = true;
              }
            }
            if (pushed) return this.UPDATE_PROPERTY_VALUE();
          },

          // combined these 2 api endpoints so they will get tracked under the same key, can revisit this later...
          async UPDATE_PROPERTY_VALUE() {
            if (this.batchedProps.length === 0) return;
            if (changeSetsStore.creatingChangeSet)
              throw new Error("race, wait until the change set is created");
            if (changeSetId === changeSetsStore.headChangeSetId)
              changeSetsStore.creatingChangeSet = true;

            const inserts = [] as InsertPropertyEditorValueArgs[];
            const updates = [] as UpdatePropertyEditorValueArgs[];

            const components = new Set(
              this.batchedProps.map((b) => b.componentId),
            );
            if (components.size > 1) {
              throw Error(
                `Cannot batch different components: ${[...components].join(
                  ", ",
                )}`,
              );
            }
            const componentId = components.values().next().value;
            // splicing so we don't re-submit changes, or drop changes that get put in after we fire
            for (const payload of this.batchedProps.splice(0)) {
              // If the valueid for this update does not exist in the values tree,
              // we shouldn't perform the update!
              const isUpdate = "attributeValueId" in payload;
              if (
                this.values?.values[
                  isUpdate
                    ? payload.attributeValueId
                    : payload.parentAttributeValueId
                ] === undefined
              ) {
                continue;
              }
              if (isUpdate) updates.push(payload);
              else inserts.push(payload);
            }

            return new ApiRequest<{ success: true }>({
              method: "post",
              url: "component/upsert_property_editor_value",
              params: {
                componentId,
                inserts,
                updates,
                ...visibilityParams,
              },
            });
          },
          async SET_COMPONENT_TYPE(payload: SetTypeArgs) {
            if (changeSetsStore.creatingChangeSet)
              throw new Error("race, wait until the change set is created");
            if (changeSetId === changeSetsStore.headChangeSetId)
              changeSetsStore.creatingChangeSet = true;

            return new ApiRequest<{ success: true }>({
              method: "post",
              url: "component/set_type",
              params: {
                ...payload,
                ...visibilityParams,
              },
              // onSuccess() {},
            });
          },
          async RESET_PROPERTY_VALUE(
            resetPayload: ResetPropertyEditorValueArgs,
          ) {
            if (changeSetsStore.creatingChangeSet)
              throw new Error("race, wait until the change set is created");
            if (changeSetId === changeSetsStore.headChangeSetId)
              changeSetsStore.creatingChangeSet = true;
            return new ApiRequest<{ success: true }>({
              method: "post",
              url: "component/restore_default_function",
              params: {
                ...resetPayload,
                ...visibilityParams,
              },
            });
          },
        },
        debounce: {
          UPDATE_PROPERTY_VALUE: [
            1000,
            {
              leading: false,
              wait: 1000,
            },
          ],
        },
        onActivated() {
          this.reloadPropertyEditorData();

          const realtimeStore = useRealtimeStore();
          realtimeStore.subscribe(this.$id, `changeset/${changeSetId}`, [
            {
              eventType: "ComponentUpdated",
              debounce: true,
              callback: (updated) => {
                if (updated.changeSetId !== changeSetId) return;
                if (updated.componentId !== this.selectedComponentId) return;
                this.reloadPropertyEditorData();
              },
            },
          ]);
          realtimeStore.subscribe(this.$id, `changeset/${changeSetId}`, [
            {
              eventType: "ChangeSetWritten",
              debounce: true,
              callback: (writtenChangeSetId) => {
                if (writtenChangeSetId !== changeSetId) return;
                this.reloadPropertyEditorData();
              },
            },
          ]);

          return () => {
            realtimeStore.unsubscribe(this.$id);
          };
        },
      },
    ),
  )();
};
