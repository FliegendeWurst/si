import { defineStore } from "pinia";
import { ApiRequest, addStoreHooks } from "@si/vue-lib/pinia";
import { useWorkspacesStore } from "@/store/workspaces.store";
import { useChangeSetsStore } from "@/store/change_sets.store";
import { Visibility } from "@/api/sdf/dal/visibility";
import { nilId } from "@/utils/nilId";

export type NodeKind = "Category" | "Content" | "Func" | "Ordering" | "Prop";

export type ContentKind =
  | "Root"
  | "ActionPrototype"
  | "AttributePrototype"
  | "AttributePrototypeArgument"
  | "AttributeValue"
  | "Component"
  | "ExternalProvider"
  | "FuncArg"
  | "Func"
  | "InternalProvider"
  | "Prop"
  | "Schema"
  | "SchemaVariant"
  | "StaticArgumentValue"
  | "ValidationPrototype";

export interface VizResponse {
  edges: {
    from: string;
    to: string;
  }[];

  nodes: {
    id: string;
    nodeKind: NodeKind;
    contentKind: ContentKind | null;
    name: string | null;
  }[];

  rootNodeId: string;
}

export const useVizStore = () => {
  const changeSetStore = useChangeSetsStore();
  const selectedChangeSetId = changeSetStore.selectedChangeSetId;
  const workspacesStore = useWorkspacesStore();
  const workspaceId = workspacesStore.selectedWorkspacePk;
  const visibility: Visibility = {
    visibility_change_set_pk: selectedChangeSetId ?? nilId(),
  };

  return addStoreHooks(
    defineStore(
      `ws${workspaceId || "NONE"}/cs${selectedChangeSetId || "NONE"}/viz`,
      {
        state: () => ({
          edges: [],
          nodes: [],
        }),
        getters: {
          nodes: (state) => state.nodes,
          edges: (state) => state.edges,
        },
        actions: {
          async FETCH_VIZ() {
            return new ApiRequest<VizResponse>({
              url: "/graphviz/nodes_edges",
              params: { ...visibility },
            });
          },
          async FETCH_SCHEMA_VARIANT_VIZ(schemaVariantId: string) {
            return new ApiRequest<VizResponse>({
              url: "/graphviz/schema_variant",
              params: { schemaVariantId, ...visibility },
            });
          },
        },
      },
    ),
  )();
};
