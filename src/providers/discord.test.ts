import { describe, expect, it } from "vitest";
import { discordSnapshotView } from "./discord";
import type { BootstrapResponse, BootstrapSnapshot } from "./contract";
import { lifecycleContext } from "../test/lifecycle-wire";
import { providerKey } from "./identity";

describe("Discord application projection", () => {
  it("shows the imported archive account instead of a generic label", () => {
    const response: BootstrapResponse<BootstrapSnapshot> = {
      contractVersion: 2,
      context: { ...lifecycleContext, scope: { ...lifecycleContext.scope, provider: providerKey("discord") } },
      payload: {
        identity: { state: "ready" },
        auth: { schema: "discord.account", version: 1, payload: { accountLabel: "Ada Example" } },
        catalog: { phase: "ready", total: 0, processed: 0 },
        chats: [], recentJobs: [], legacyHistory: []
      }
    };

    expect(discordSnapshotView(response).accountLabel).toBe("Ada Example");
  });
});
