import * as _ from "lodash-es";
import { defineStore } from "pinia";
import { watch } from "vue";
import { addStoreHooks, ApiRequest } from "@si/vue-lib/pinia";

import storage from "local-storage-fallback"; // drop-in storage polyfill which falls back to cookies/memory
import { Visibility } from "@/api/sdf/dal/visibility";
import {
  FuncArgument,
  FuncArgumentKind,
  FuncKind,
  FuncId,
  FuncArgumentId,
} from "@/api/sdf/dal/func";

import { nilId } from "@/utils/nilId";
import { trackEvent } from "@/utils/tracking";
import keyedDebouncer from "@/utils/keyedDebouncer";
import { useWorkspacesStore } from "@/store/workspaces.store";
import { useAssetStore } from "@/store/asset.store";
import { PropId } from "@/api/sdf/dal/prop";
import { SchemaVariantId, OutputSocketId } from "@/api/sdf/dal/schema";
import { useChangeSetsStore } from "../change_sets.store";
import { useRealtimeStore } from "../realtime/realtime.store";
import { useComponentsStore } from "../components.store";

import {
  AttributePrototypeBag,
  AttributePrototypeArgumentBag,
  CreateFuncOptions,
  FuncAssociations,
  InputSocketView,
  InputSourceProp,
  OutputLocation,
  OutputSocketView,
} from "./types";

import { FuncRunId } from "../func_runs.store";

export type FuncSummary = {
  id: string;
  kind: FuncKind;
  name: string;
  displayName?: string;
  description?: string;
  isBuiltin: boolean;
};

export type FuncWithDetails = FuncSummary & {
  code: string;
  types: string;
  associations?: FuncAssociations;
};

type FuncExecutionState =
  | "Create"
  | "Dispatch"
  | "Failure"
  | "Run"
  | "Start"
  | "Success";

// TODO: remove when fn log stuff gets figured out a bit deeper
/* eslint-disable @typescript-eslint/no-explicit-any */
export type FuncExecutionLog = {
  id: FuncId;
  state: FuncExecutionState;
  value?: any;
  outputStream?: any[];
  functionFailure?: any; // FunctionResultFailure
};

export interface SaveFuncResponse {
  types: string;
  associations?: FuncAssociations;
}

export interface DeleteFuncResponse {
  success: boolean;
}

export interface OutputLocationOption {
  label: string;
  value: OutputLocation;
}

const LOCAL_STORAGE_FUNC_IDS_KEY = "si-open-func-ids";

export type InputSourceProps = { [key: string]: InputSourceProp[] };
export type InputSocketViews = { [key: string]: InputSocketView[] };
export type OutputSocketViews = { [key: string]: OutputSocketView[] };

export const useFuncStore = () => {
  const componentsStore = useComponentsStore();
  const changeSetsStore = useChangeSetsStore();
  const selectedChangeSetId = changeSetsStore.selectedChangeSet?.id;

  // TODO(nick): we need to allow for empty visibility here. Temporarily send down "nil" to mean that we want the
  // query to find the default change set.
  const visibility: Visibility = {
    visibility_change_set_pk:
      selectedChangeSetId ?? changeSetsStore.headChangeSetId ?? nilId(),
  };

  const workspacesStore = useWorkspacesStore();
  const workspaceId = workspacesStore.selectedWorkspacePk;

  let funcSaveDebouncer: ReturnType<typeof keyedDebouncer> | undefined;

  return addStoreHooks(
    defineStore(`ws${workspaceId || "NONE"}/cs${selectedChangeSetId}/funcs`, {
      state: () => ({
        funcsById: {} as Record<FuncId, FuncSummary>,
        funcArgumentsById: {} as Record<FuncArgumentId, FuncArgument>,
        funcArgumentsByFuncId: {} as Record<FuncId, FuncArgument[]>,
        funcDetailsById: {} as Record<FuncId, FuncWithDetails>,
        // map from schema variant ids to the input sources
        inputSourceSockets: {} as InputSocketViews,
        inputSourceProps: {} as InputSourceProps,
        outputSockets: {} as OutputSocketViews,
        openFuncIds: [] as FuncId[],
        lastFuncExecutionLogByFuncId: {} as Record<FuncId, FuncExecutionLog>,
        // represents the last, or "focused" func clicked on/open by the editor
        selectedFuncId: undefined as FuncId | undefined,
      }),
      getters: {
        selectedFuncSummary(state): FuncSummary | undefined {
          return state.funcsById[this.selectedFuncId || ""];
        },
        selectedFuncDetails(state): FuncWithDetails | undefined {
          return state.funcDetailsById[this.selectedFuncId || ""];
        },
        funcArguments(state): FuncArgument[] | undefined {
          return state.selectedFuncId
            ? state.funcArgumentsByFuncId[state.selectedFuncId]
            : undefined;
        },

        nameForSchemaVariantId: (_state) => (schemaVariantId: string) =>
          componentsStore.schemaVariantsById[schemaVariantId]?.schemaName,

        funcById: (state) => (funcId: FuncId) => state.funcDetailsById[funcId],

        funcList: (state) => _.values(state.funcsById),

        allProps: (state) =>
          _.reduce(
            state.inputSourceProps,
            (acc, props) => [...acc, ...props],
            [] as InputSourceProp[],
          ),

        propsForId(): Record<PropId, InputSourceProp> {
          return _.keyBy(this.allProps, (s) => s.propId);
        },

        inputSocketForId:
          (state) =>
          (inputSocketId: string): InputSocketView | undefined => {
            for (const sockets of Object.values(state.inputSourceSockets)) {
              const inputSourceSocket = sockets.find(
                (socket) => socket.inputSocketId === inputSocketId,
              );
              if (inputSourceSocket) {
                return inputSourceSocket;
              }
            }
            return undefined;
          },

        allOutputSockets: (state) => {
          return _.reduce(
            state.outputSockets,
            (acc, sockets) => [...acc, ...sockets],
            [] as OutputSocketView[],
          );
        },

        outputSocketsForId(): Record<OutputSocketId, OutputSocketView> {
          return _.keyBy(this.allOutputSockets, (s) => s.outputSocketId);
        },

        schemaVariantIdForPrototypeTargetId(): Record<
          OutputSocketId | PropId,
          SchemaVariantId
        > {
          const propsBySvId = _.mapValues(
            this.propsForId,
            (p) => p.schemaVariantId,
          );
          const outputSocketBySvId = _.mapValues(
            this.outputSocketsForId,
            (s) => s.schemaVariantId,
          );

          return _.merge({}, propsBySvId, outputSocketBySvId);
        },

        // Filter props by schema variant
        propsAsOptionsForSchemaVariant: (state) => (schemaVariantId: string) =>
          (schemaVariantId === nilId()
            ? _.flatten(Object.values(state.inputSourceProps))
            : state.inputSourceProps[schemaVariantId]
          )?.map((prop) => ({
            label: prop.path,
            value: prop.propId,
          })) ?? [],

        schemaVariantOptions() {
          return componentsStore.schemaVariants.map((sv) => ({
            label: sv.schemaName,
            value: sv.schemaVariantId,
          }));
        },

        componentOptions(): { label: string; value: string }[] {
          return componentsStore.allComponents.map(
            ({ displayName, id, schemaVariantId }) => ({
              label: `${displayName} (${
                this.nameForSchemaVariantId(schemaVariantId) ?? "unknown"
              })`,
              value: id,
            }),
          );
        },
      },

      actions: {
        propIdToSourceName(propId: string) {
          const prop = this.propsForId[propId];
          if (prop) {
            return `Attribute: ${prop.path}`;
          }
        },
        inputSocketIdToSourceName(inputSocketId: string) {
          const socket = this.inputSocketForId(inputSocketId);
          if (socket) {
            return `Input Socket: ${socket.name}`;
          }
        },

        outputSocketIdToSourceName(outputSocketId: string) {
          const outputSocket = this.outputSocketsForId[outputSocketId];
          if (outputSocket) {
            return `Output Socket: ${outputSocket.name}`;
          }
          return undefined;
        },

        outputLocationForAttributePrototype(
          prototype: AttributePrototypeBag,
        ): OutputLocation | undefined {
          if (prototype.propId) {
            return {
              label: this.propIdToSourceName(prototype.propId) ?? "none",
              propId: prototype.propId,
            };
          }

          if (prototype.outputSocketId) {
            return {
              label:
                this.outputSocketIdToSourceName(prototype.outputSocketId) ??
                "none",
              outputSocketId: prototype.outputSocketId,
            };
          }

          return undefined;
        },

        outputLocationOptionsForSchemaVariant(
          schemaVariantId: string,
        ): OutputLocationOption[] {
          const propOptions =
            (schemaVariantId === nilId()
              ? _.flatten(Object.values(this.inputSourceProps))
              : this.inputSourceProps[schemaVariantId]
            )
              ?.filter((p) => p.eligibleForOutput)
              .map((prop) => {
                const label = this.propIdToSourceName(prop.propId) ?? "none";
                return {
                  label,
                  value: {
                    label,
                    propId: prop.propId,
                  },
                };
              }) ?? [];

          const socketOptions =
            (schemaVariantId === nilId()
              ? _.flatten(Object.values(this.outputSockets))
              : this.outputSockets[schemaVariantId]
            )?.map((socket) => {
              const label =
                this.outputSocketIdToSourceName(socket.outputSocketId) ??
                "none";
              return {
                label,
                value: {
                  label,
                  outputSocketId: socket.outputSocketId,
                },
              };
            }) ?? [];
          return [...propOptions, ...socketOptions];
        },

        async FETCH_FUNC_LIST() {
          return new ApiRequest<{ funcs: FuncSummary[] }, Visibility>({
            url: "func/list_funcs",
            params: {
              ...visibility,
            },
            onSuccess: (response) => {
              this.funcsById = _.keyBy(response.funcs, (f) => f.id);
              this.recoverOpenFuncIds();
            },
          });
        },
        async FETCH_FUNC(funcId: FuncId) {
          return new ApiRequest<FuncWithDetails>({
            url: "func/get_func",
            params: {
              id: funcId,
              ...visibility,
            },
            keyRequestStatusBy: funcId,
            onSuccess: (response) => {
              this.funcDetailsById[response.id] = response;
            },
          });
        },
        async FETCH_FUNC_ASSOCIATIONS(funcId: FuncId) {
          return new ApiRequest<{ associations?: FuncAssociations }>({
            url: "func/get_func_associations",
            params: {
              id: funcId,
              ...visibility,
            },
            keyRequestStatusBy: funcId,
            onSuccess: (response) => {
              const func = this.funcDetailsById[funcId];
              if (func) {
                func.associations = response.associations;
                this.funcDetailsById[funcId] = func;
              }
            },
          });
        },
        async DELETE_FUNC(funcId: FuncId) {
          return new ApiRequest<DeleteFuncResponse>({
            method: "post",
            url: "func/delete_func",
            params: {
              id: funcId,
              ...visibility,
            },
          });
        },
        async UPDATE_FUNC(func: FuncWithDetails) {
          if (changeSetsStore.creatingChangeSet)
            throw new Error("race, wait until the change set is created");
          if (changeSetsStore.headSelected)
            changeSetsStore.creatingChangeSet = true;
          const isHead = changeSetsStore.headSelected;

          return new ApiRequest<SaveFuncResponse>({
            method: "post",
            url: "func/save_func",
            params: {
              ...func,
              ...visibility,
            },
            optimistic: () => {
              if (isHead) return () => {};

              const current = this.funcById(func.id);
              this.funcDetailsById[func.id] = {
                ...func,
                code: current?.code ?? func.code,
              };
              return () => {
                if (current) {
                  this.funcDetailsById[func.id] = current;
                } else {
                  delete this.funcDetailsById[func.id];
                }
              };
            },
            onFail: () => {
              changeSetsStore.creatingChangeSet = false;
            },
            keyRequestStatusBy: func.id,
          });
        },
        async CREATE_ATTRIBUTE_PROTOTYPE(
          funcId: FuncId,
          schemaVariantId: string,
          prototypeArguments: AttributePrototypeArgumentBag[],
          componentId?: string,
          propId?: string,
          outputSocketId?: string,
        ) {
          if (changeSetsStore.creatingChangeSet)
            throw new Error("race, wait until the change set is created");
          if (changeSetsStore.headSelected)
            changeSetsStore.creatingChangeSet = true;

          return new ApiRequest<null>({
            method: "post",
            url: "func/create_attribute_prototype",
            params: {
              funcId,
              schemaVariantId,
              componentId,
              propId,
              outputSocketId,
              prototypeArguments,
              ...visibility,
            },
            onFail: () => {
              changeSetsStore.creatingChangeSet = false;
            },
          });
        },
        async UPDATE_ATTRIBUTE_PROTOTYPE(
          funcId: FuncId,
          attributePrototypeId: string,
          prototypeArguments: AttributePrototypeArgumentBag[],
          propId?: string,
          outputSocketId?: string,
        ) {
          if (changeSetsStore.creatingChangeSet)
            throw new Error("race, wait until the change set is created");
          if (changeSetsStore.headSelected)
            changeSetsStore.creatingChangeSet = true;

          return new ApiRequest<null>({
            method: "post",
            url: "func/update_attribute_prototype",
            params: {
              funcId,
              attributePrototypeId,
              propId,
              outputSocketId,
              prototypeArguments,
              ...visibility,
            },
            onFail: () => {
              changeSetsStore.creatingChangeSet = false;
            },
          });
        },
        async REMOVE_ATTRIBUTE_PROTOTYPE(attributePrototypeId: string) {
          if (changeSetsStore.creatingChangeSet)
            throw new Error("race, wait until the change set is created");
          if (changeSetsStore.headSelected)
            changeSetsStore.creatingChangeSet = true;

          return new ApiRequest<null>({
            method: "post",
            url: "func/remove_attribute_prototype",
            params: {
              attributePrototypeId,
              ...visibility,
            },
            onFail: () => {
              changeSetsStore.creatingChangeSet = false;
            },
          });
        },
        async CREATE_FUNC_ARGUMENT(
          funcId: FuncId,
          name: string,
          kind: FuncArgumentKind,
          elementKind?: FuncArgumentKind,
        ) {
          if (changeSetsStore.creatingChangeSet)
            throw new Error("race, wait until the change set is created");
          if (changeSetsStore.headSelected)
            changeSetsStore.creatingChangeSet = true;

          return new ApiRequest<null>({
            method: "post",
            url: "func/create_func_argument",
            params: {
              funcId,
              name,
              kind,
              elementKind,
              ...visibility,
            },
            onFail: () => {
              changeSetsStore.creatingChangeSet = false;
            },
          });
        },
        async UPDATE_FUNC_ARGUMENT(
          funcId: FuncId,
          funcArgumentId: FuncArgumentId,
          name: string,
          kind: FuncArgumentKind,
          elementKind?: FuncArgumentKind,
        ) {
          if (changeSetsStore.creatingChangeSet)
            throw new Error("race, wait until the change set is created");
          if (changeSetsStore.headSelected)
            changeSetsStore.creatingChangeSet = true;

          return new ApiRequest<null>({
            method: "post",
            url: "func/update_func_argument",
            params: {
              funcId,
              funcArgumentId,
              name,
              kind,
              elementKind,
              ...visibility,
            },
            onFail: () => {
              changeSetsStore.creatingChangeSet = false;
            },
          });
        },
        async DELETE_FUNC_ARGUMENT(
          funcId: FuncId,
          funcArgumentId: FuncArgumentId,
        ) {
          if (changeSetsStore.creatingChangeSet)
            throw new Error("race, wait until the change set is created");
          if (changeSetsStore.headSelected)
            changeSetsStore.creatingChangeSet = true;

          return new ApiRequest<null>({
            method: "post",
            url: "func/delete_func_argument",
            params: {
              funcId,
              funcArgumentId,
              ...visibility,
            },
            onFail: () => {
              changeSetsStore.creatingChangeSet = false;
            },
          });
        },
        async FETCH_FUNC_ARGUMENT_LIST(funcId: FuncId) {
          return new ApiRequest<{ funcArguments: FuncArgument[] }>({
            url: "func/list_func_arguments",
            params: {
              funcId,
              ...visibility,
            },
            onSuccess: (response) => {
              this.funcArgumentsByFuncId[funcId] = response.funcArguments;
              for (const argument of response.funcArguments) {
                this.funcArgumentsById[argument.id] = argument;
              }
            },
          });
        },
        async SAVE_AND_EXEC_FUNC(funcId: FuncId) {
          const func = this.funcById(funcId);
          if (func) {
            trackEvent("func_save_and_exec", { id: func.id, name: func.name });
          }

          if (changeSetsStore.creatingChangeSet)
            throw new Error("race, wait until the change set is created");
          if (changeSetsStore.headSelected)
            changeSetsStore.creatingChangeSet = true;

          return new ApiRequest<SaveFuncResponse>({
            method: "post",
            url: "func/save_and_exec",
            keyRequestStatusBy: funcId,
            params: { ...func, ...visibility },
            onFail: () => {
              changeSetsStore.creatingChangeSet = false;
            },
          });
        },
        async TEST_EXECUTE(executeRequest: {
          id: FuncId;
          args: unknown;
          code: string;
          componentId: string;
        }) {
          const func = this.funcById(executeRequest.id);
          if (func) {
            trackEvent("function_test_execute", {
              id: func.id,
              name: func.name,
            });
          }

          return new ApiRequest<{
            funcRunId: FuncRunId;
          }>({
            method: "post",
            url: "func/test_execute",
            params: { ...executeRequest, ...visibility },
          });
        },
        async CREATE_FUNC(createFuncRequest: {
          kind: FuncKind;
          name?: string;
          options?: CreateFuncOptions;
        }) {
          if (changeSetsStore.creatingChangeSet)
            throw new Error("race, wait until the change set is created");
          if (changeSetsStore.headSelected)
            changeSetsStore.creatingChangeSet = true;

          return new ApiRequest<FuncSummary>({
            method: "post",
            url: "func/create_func",
            params: { ...createFuncRequest, ...visibility },
            onSuccess: (response) => {
              this.funcsById[response.id] = response;
            },
            onFail: () => {
              changeSetsStore.creatingChangeSet = false;
            },
          });
        },
        async FETCH_INPUT_SOURCE_LIST(schemaVariantId?: string) {
          return new ApiRequest<{
            inputSockets: InputSocketView[];
            outputSockets: OutputSocketView[];
            props: InputSourceProp[];
          }>({
            url: "func/list_input_sources",
            params: { schemaVariantId, ...visibility },
            onSuccess: (response) => {
              const inputSourceSockets = this.inputSourceSockets;
              const inputSourceSocketsFromResponse = _.groupBy(
                response.inputSockets,
                "schemaVariantId",
              );
              for (const schemaVariantId in inputSourceSocketsFromResponse) {
                inputSourceSockets[schemaVariantId] =
                  inputSourceSocketsFromResponse[schemaVariantId] ?? [];
              }
              this.inputSourceSockets = inputSourceSockets;

              const inputSourceProps = this.inputSourceProps;
              const inputSourcePropsFromResponse = _.groupBy(
                response.props,
                "schemaVariantId",
              );
              for (const _schemaVariantId in inputSourcePropsFromResponse) {
                inputSourceProps[_schemaVariantId] =
                  inputSourcePropsFromResponse[_schemaVariantId] ?? [];
              }
              this.inputSourceProps = inputSourceProps;

              const outputSockets = this.outputSockets;
              const outputSocketsFromResponse = _.groupBy(
                response.outputSockets,
                "schemaVariantId",
              );
              for (const _schemaVariantId in outputSocketsFromResponse) {
                outputSockets[_schemaVariantId] =
                  outputSocketsFromResponse[_schemaVariantId] ?? [];
              }
              this.outputSockets = outputSockets;
            },
          });
        },
        async FETCH_PROTOTYPE_ARGUMENTS(
          propId?: string,
          outputSocketId?: string,
        ) {
          return new ApiRequest<{
            preparedArguments: Record<string, unknown>;
          }>({
            url: "attribute/get_prototype_arguments",
            params: { propId, outputSocketId, ...visibility },
          });
        },

        async recoverOpenFuncIds() {
          // fetch the list of open funcs from localstorage
          const localStorageFuncIds = (
            storage.getItem(LOCAL_STORAGE_FUNC_IDS_KEY) ?? ""
          ).split(",") as FuncId[];
          // Filter out cached ids that don't correspond to funcs anymore
          const newOpenFuncIds = _.intersection(
            localStorageFuncIds,
            _.keys(this.funcsById),
          );
          if (!_.isEqual(newOpenFuncIds, this.openFuncIds)) {
            this.openFuncIds = newOpenFuncIds;
          }
        },

        setOpenFuncId(id: FuncId, isOpen: boolean, unshift?: boolean) {
          if (isOpen) {
            if (!this.openFuncIds.includes(id)) {
              this.openFuncIds[unshift ? "unshift" : "push"](id);
            }
          } else {
            const funcIndex = _.indexOf(this.openFuncIds, id);
            if (funcIndex >= 0) this.openFuncIds.splice(funcIndex, 1);
          }

          storage.setItem(
            LOCAL_STORAGE_FUNC_IDS_KEY,
            this.openFuncIds.join(","),
          );
        },

        updateFuncCode(funcId: FuncId, code: string) {
          const func = _.cloneDeep(this.funcDetailsById[funcId]);
          if (!func || func.code === code) return;
          func.code = code;

          this.enqueueFuncSave(func);
        },

        enqueueFuncSave(func: FuncWithDetails) {
          if (changeSetsStore.headSelected) return this.UPDATE_FUNC(func);

          this.funcDetailsById[func.id] = func;

          // Lots of ways to handle this... we may want to handle this debouncing in the component itself
          // so the component has its own "draft" state that it passes back to the store when it's ready to save
          // however this should work for now, and lets the store handle this logic
          if (!funcSaveDebouncer) {
            funcSaveDebouncer = keyedDebouncer((id: FuncId) => {
              const f = this.funcDetailsById[id];
              if (!f) return;
              this.UPDATE_FUNC(f);
            }, 500);
          }
          // call debounced function which will trigger sending the save to the backend
          const saveFunc = funcSaveDebouncer(func.id);
          if (saveFunc) {
            saveFunc(func.id);
          }
        },
      },
      onActivated() {
        this.FETCH_FUNC_LIST();
        this.FETCH_INPUT_SOURCE_LIST();

        // could do this from components, but may as well do here...
        const stopWatchSelectedFunc = watch([() => this.selectedFuncId], () => {
          if (this.selectedFuncId) {
            // only fetch if we don't have this one already in our state,
            // otherwise we can overwrite functions with their previous value
            // before the save queue is drained.
            if (
              typeof this.funcDetailsById[this.selectedFuncId] === "undefined"
            ) {
              this.FETCH_FUNC(this.selectedFuncId);
            }

            // add the func to the list of open ones
            this.setOpenFuncId(this.selectedFuncId, true);
          }
        });

        const assetStore = useAssetStore();
        const realtimeStore = useRealtimeStore();

        realtimeStore.subscribe(this.$id, `changeset/${selectedChangeSetId}`, [
          {
            eventType: "ChangeSetWritten",
            callback: () => {
              this.FETCH_FUNC_LIST();
            },
          },
          // TODO(victor) we don't need the changeSetId checks below, since nats filters messages already
          {
            eventType: "FuncCreated",
            callback: (data) => {
              if (data.changeSetId !== selectedChangeSetId) return;
              this.FETCH_FUNC_LIST();
            },
          },
          {
            eventType: "FuncDeleted",
            callback: (data) => {
              if (data.changeSetId !== selectedChangeSetId) return;
              this.FETCH_FUNC_LIST();

              const assetId = assetStore.selectedAssetId;
              if (
                assetId &&
                this.selectedFuncId &&
                assetStore.selectedFuncs.includes(this.selectedFuncId)
              ) {
                assetStore.closeFunc(assetId, this.selectedFuncId);
              }
            },
          },
          {
            eventType: "FuncArgumentsSaved",
            callback: (data) => {
              if (data.changeSetId !== selectedChangeSetId) return;
              if (data.funcId !== this.selectedFuncId) return;
              this.FETCH_FUNC_ARGUMENT_LIST(data.funcId);
              this.FETCH_FUNC(data.funcId);
            },
          },
          {
            eventType: "FuncSaved",
            callback: (data) => {
              if (data.changeSetId !== selectedChangeSetId) return;
              this.FETCH_FUNC_LIST();

              // Reload the last selected asset to ensure that its func list is up to date.
              const assetId = assetStore.selectedAssetId;
              if (assetId) {
                assetStore.LOAD_ASSET(assetId);
              }

              if (this.selectedFuncId) {
                // Only fetch if we don't have the selected func in our state or if we are on HEAD.
                // If we are on HEAD, the func is immutable, so we are safe to fetch. However, if
                // we are not on HEAD, then the func is mutable. Therefore, we can only fetch
                // relevant metadata in order to avoid overwriting functions with their previous
                // value before the save queue is drained.
                if (data.funcId === this.selectedFuncId) {
                  if (
                    typeof this.funcDetailsById[this.selectedFuncId] ===
                      "undefined" ||
                    changeSetsStore.headSelected
                  ) {
                    this.FETCH_FUNC(this.selectedFuncId);
                  } else {
                    this.FETCH_FUNC_ASSOCIATIONS(this.selectedFuncId);
                  }
                }
              }
            },
          },
        ]);
        return () => {
          stopWatchSelectedFunc();
          realtimeStore.unsubscribe(this.$id);
        };
      },
    }),
  )();
};
