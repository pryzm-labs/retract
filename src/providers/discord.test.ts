import { describe, expect, it } from "vitest";
import { discordConversationView, discordSnapshotView } from "./discord";
import type { BootstrapResponse, BootstrapSnapshot } from "./contract";
import { decodeConversation } from "./contract";
import fixture from "../test/fixtures/provider-lifecycle.json";
import { lifecycleContext, wireChat } from "../test/lifecycle-wire";
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

  it("uses Discord recipients and guild metadata for safe navigation labels", () => {
    const record = decodeConversation(wireChat(fixture.chats[0]), lifecycleContext.scope);
    const direct = discordConversationView({
      ...record,
      title: "",
      providerMetadata: { schema: "discord.conversation_metadata", version: 1, payload: {
        sourceType: "opaque", channelName: null, verifiedKind: "other", guild: null,
        recipients: ["Ada Example", "Bob"], warnings: ["unknown_conversation_kind"]
      } }
    }, "Ada Example");
    expect(direct.title).toBe("Bob");
    expect(direct.discordNavigation).toEqual({ category: "direct", groupId: null, groupLabel: null, detail: "Direct message" });

    const server = discordConversationView({
      ...record,
      title: "general",
      providerMetadata: { schema: "discord.conversation_metadata", version: 1, payload: {
        sourceType: "opaque", channelName: "general", verifiedKind: "other",
        guild: { id: "123", name: "Pryzm Labs" }, recipients: null, warnings: ["unknown_conversation_kind"]
      } }
    }, "Ada Example");
    expect(server.title).toBe("general");
    expect(server.discordNavigation).toEqual({ category: "server", groupId: "123", groupLabel: "Pryzm Labs", detail: "Pryzm Labs" });
  });

  it("never leaves an unclassified Discord conversation with a blank title", () => {
    const record = decodeConversation(wireChat(fixture.chats[0]), lifecycleContext.scope);
    const chat = discordConversationView({
      ...record,
      title: "",
      providerMetadata: { schema: "discord.conversation_metadata", version: 1, payload: {
        sourceType: "opaque", channelName: null, verifiedKind: "other", guild: null,
        recipients: null, warnings: ["unknown_conversation_kind"]
      } }
    }, "Ada Example");
    expect(chat.title).toBe("Unknown Discord chat");
    expect(chat.discordNavigation).toEqual({ category: "other", groupId: null, groupLabel: null, detail: "Unclassified chat" });
  });
});
