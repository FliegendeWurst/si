import { defineStore } from "pinia";
import { ApiRequest } from "@si/vue-lib/pinia";
import { Workspace, WorkspaceId } from "./workspaces.store";
import api from "./api";
import {
  ApiBuilder,
  ApiEndpoint,
  ApiRequestDebouncer,
  ApiRequestDescription,
  BaseRequestParams,
  URLPattern,
} from "@si/vue-lib/src/utils/api_debouncer";
import { AxiosInstance } from "axios";

export type AuthTokenId = string;
export interface AuthToken {
  id: string;
  name: string | null;
  userId: string;
  workspaceId: string;
  createdAt: Date;
  expiresAt: Date | null;
  claims: unknown;
  lastUsedAt: Date | null;
  lastUsedIp: string | null;
}

type WorkspaceUrl = ["workspaces", { workspaceId: WorkspaceId }];
type AuthApiEndpoints =
  | {
      url: ["workspaces"];
      response: Workspace[];
    }
  | {
      url: [...WorkspaceUrl];
      response: Workspace;
    }
  | {
      url: [...WorkspaceUrl, "authTokens"];
      response: AuthToken[];
    }
  | {
      url: [...WorkspaceUrl, "authTokens", { tokenId: AuthTokenId }];
      response: AuthToken;
    }
  | {
      url: [...WorkspaceUrl, "authTokens", { tokenId: AuthTokenId }];
      method: "put";
      params: { name: string | null };
      response: AuthToken;
    };

type RequestParamsWithDefault<P extends BaseRequestParams | undefined> =
  | Exclude<P, undefined>
  | (undefined extends Extract<P, undefined> ? BaseRequestParams : never);

/**
 * Caches and debounces all api requests.
 *
 * Uses the passed-in API endpoint definitions to drive the request and response types.
 */
function useDebouncedApi<Api extends Endpoint>(api: AxiosInstance) {
  const endpoints = new Map<
    { url: Api["url"]; method?: Api["method"] },
    ApiEndpoint
  >();
  return <E extends { url: Api["url"]; method?: Api["method"] }>(
    endpointSpec: E,
  ) => {
    if (!endpoints.has(endpointSpec))
      endpoints.set(
        endpointSpec,
        new ApiEndpoint(api, endpointSpec.url, endpointSpec.method),
      );
    return endpoints.get(endpointSpec) as ApiEndpoint<
      Extract<Api, E>["response"],
      RequestParamsWithDefault<Extract<Api, E>["params"]>
    >;
  };
}
const f = useDebouncedApi<AuthApiEndpoints>(api);
const f2 = f({
  url: ["workspaces", { workspaceId: "blah" }, "authTokens", { tokenId: "x" }],
});
async function blah() {
  const r = await f2.fetch();
}
type Endpoint = {
  url: URLPattern;
  method?: ApiRequestDescription["method"];
  response: unknown;
  params?: BaseRequestParams;
};

export const useAuthTokensApi = defineStore("authTokens", {
  actions: {
    async FETCH_AUTH_TOKENS(workspaceId: WorkspaceId) {
      return new ApiRequest<{ authTokens: AuthToken[] }>({
        url: ["workspaces", { workspaceId }, "authTokens"],
        keyRequestStatusBy: workspaceId,
      });
    },

    async CREATE_AUTH_TOKEN(workspaceId: WorkspaceId, name?: string) {
      return new ApiRequest<{ authToken: AuthToken; token: string }>({
        method: "post",
        url: ["workspaces", { workspaceId }, "authTokens"],
        params: { name },
        keyRequestStatusBy: workspaceId,
      });
    },

    async FETCH_AUTH_TOKEN(workspaceId: WorkspaceId, tokenId: AuthTokenId) {
      return new ApiRequest<{ authToken: AuthToken }>({
        url: ["workspaces", { workspaceId }, "authTokens", { tokenId }],
        keyRequestStatusBy: [workspaceId, tokenId],
      });
    },

    async RENAME_AUTH_TOKEN(
      workspaceId: WorkspaceId,
      tokenId: AuthTokenId,
      name: string | null,
    ) {
      return new ApiRequest<void>({
        method: "put",
        url: ["workspaces", { workspaceId }, "authTokens", { tokenId }],
        params: { name },
        keyRequestStatusBy: [workspaceId, tokenId],
      });
    },

    async REVOKE_AUTH_TOKEN(workspaceId: WorkspaceId, tokenId: AuthTokenId) {
      return new ApiRequest<void>({
        method: "delete",
        url: ["workspaces", { workspaceId }, "authTokens", { tokenId }],
        keyRequestStatusBy: [workspaceId, tokenId],
      });
    },
  },
});
