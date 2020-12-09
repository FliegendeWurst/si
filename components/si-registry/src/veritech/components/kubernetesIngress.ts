import { registry } from "@/registry";
import { EntityObject } from "@/systemComponent";
import {
  ActionRequest,
  ActionReply,
  SyncResourceRequest,
  SyncResourceReply,
  CalculatePropertiesRequest,
  CalculatePropertiesResult,
} from "../../veritech/intelligence";
import { Event } from "../../veritech/eventLog";
import { kubernetesApply, kubernetesSync } from "./kubernetesShared";
import _ from "lodash";

const intelligence = (registry.get("kubernetesIngress") as EntityObject)
  .intelligence;

intelligence.calculateProperties = function(
  req: CalculatePropertiesRequest,
): CalculatePropertiesResult {
  const result: CalculatePropertiesResult = {
    inferredProperties: {
      __baseline: {
        kubernetesObject: {
          apiVersion: "v1",
          kind: "Ingress",
        },
      },
    },
  };
  return result;
};

intelligence.syncResource = async function(
  request: SyncResourceRequest,
  event: Event,
): Promise<SyncResourceReply> {
  return await kubernetesSync(request, event);
};

intelligence.actions = {
  async apply(request: ActionRequest, event: Event): Promise<ActionReply> {
    return await kubernetesApply(request, event);
  },
};
