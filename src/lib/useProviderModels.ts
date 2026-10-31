import { useEffect, useRef, useState } from "react";

import * as api from "./api";
import type { ModelInfo } from "./api";

/**
 * The model list for the provider in force.
 *
 * `listedFor` says which provider the list on screen is for. It stops switching
 * providers from listing twice, once here and again when starting the session
 * moves `providerId` on to the new one. It also decides whose answer counts: a
 * slow provider's list arriving after a fast one's would otherwise be offered
 * under the fast one's name, and nothing would ask again.
 */
export function useProviderModels(providerId: string | null | undefined) {
  const [models, setModels] = useState<ModelInfo[] | "failed" | null>(null);
  const listedFor = useRef<string | null>(null);

  useEffect(() => {
    if (!providerId || listedFor.current === providerId) return;
    listedFor.current = providerId;
    setModels(null);
    api
      .listModels(providerId)
      .then((list) => {
        if (listedFor.current === providerId) setModels(list);
      })
      .catch(() => {
        if (listedFor.current === providerId) setModels("failed");
      });
  }, [providerId]);

  return { models, setModels, listedFor };
}
