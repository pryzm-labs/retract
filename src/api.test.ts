import { fixtureContext, fixtureRef, fixtureChatId, fixtureMessageId } from "./demo";
import { testId, testJob } from "./test/v2-fixtures";
import { uuid } from "./providers/identity";
import { beforeEach, describe, expect, it } from "vitest";
import { api, fixtureApi } from "./api.fixture";
import { hasCryptoWallet } from "./demo";

describe("browser demo API", () => {
  beforeEach(async () => {
    await fixtureApi.resetFixtures();
  });

  it("searches captions and message text without a cloud index", async () => {
    const response = await api.search({
      query: "passport apartment",
      conversations: [],
      chatKinds: [],
      contentKinds: [],
      direction: "any",
      excludePinned: false,
      limit: 100
    }, fixtureContext);
    expect(response.messages).toHaveLength(1);
    expect(response.messages[0].contentKind).toBe("photo");
  });

  it("searches a keyword across every chat when no chat scope is selected", async () => {
    const response = await api.search({
      query: "recovery phrase",
      conversations: [],
      chatKinds: [],
      contentKinds: [],
      direction: "any",
      excludePinned: false,
      limit: 100
    }, fixtureContext);
    expect(response.messages).toHaveLength(1);
    expect(response.messages[0].chatId).toBe(fixtureChatId("202"));
  });

  it("finds multiple sensitive-data categories without a keyword", async () => {
    const response = await api.search({
      query: "",
      conversations: [],
      chatKinds: [],
      contentKinds: [],
      direction: "any",
      excludePinned: false,
      privacyScan: true,
      limit: 100
    }, fixtureContext);
    const findings = new Set(response.messages.flatMap((message) => message.privacyFindings));
    expect(findings.has("email_address")).toBe(true);
    expect(findings.has("crypto_wallet")).toBe(true);
    expect(findings.has("postal_address")).toBe(true);
    expect(findings.has("precise_location")).toBe(true);
    expect(findings.has("identity_document")).toBe(true);
    expect(findings.has("credential_or_secret")).toBe(true);
    expect(findings.has("contact_card")).toBe(true);
    const walletMessage = response.messages.find((message) => message.preview.includes("0x529084"));
    expect(walletMessage?.privacyFindings).not.toContain("phone_number");
  });

  it("recognizes Ethereum, Bitcoin, and Solana wallet address formats", () => {
    const samples = [
      "ETH: 0xde709f2102306220921060314715629080e2fb77",
      "BTC: 1BoatSLRHtKNngkdXEeobR76b53LETtpyT",
      "BTC: 3J98t1WpEZ73CNmQviecrnyiWrnqRhWNLy",
      "BTC: BC1QW508D6QEJXTDG4Y5R3ZARVARY0C5XW7KV8F3T4",
      "Solana wallet: So11111111111111111111111111111111111111112"
    ];

    samples.forEach((sample) => expect(hasCryptoWallet(sample), sample).toBe(true));
    expect(hasCryptoWallet("Opaque identifier 11111111111111111111111111111111")).toBe(false);
    expect(hasCryptoWallet("Solana wallet: 1111111111111111111111111111111")).toBe(false);
    expect(hasCryptoWallet("BTC: bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t5")).toBe(false);
  });

  it("deletes only everyone-capable messages from a frozen selection", async () => {
    const plan = await api.prepareSelection([
      fixtureRef("content", "-1001", "14"),
      fixtureRef("content", "-1003", "32")
    ], fixtureContext);
    expect(plan.summary).toMatchObject({
      selected: 2,
      deleteForEveryone: 1,
      cannotDelete: 1
    });

    const job = await api.execute(plan, true, null);
    expect(job.status).toBe("completed");
    expect(job.deleted).toBe(1);
    expect(job.skipped).toBe(1);
    expect(job.dirtyRefs).toEqual([fixtureRef("conversation", "-1001")]);

    const response = await api.search({
      query: "",
      conversations: [fixtureRef("conversation", "-1001")],
      chatKinds: [],
      contentKinds: [],
      direction: "any",
      excludePinned: false,
      limit: 100
    }, fixtureContext);
    expect(response.messages.some((message) => message.messageId === fixtureMessageId("-1001", "14"))).toBe(false);
  });

  it("requires the exact group title for a critical operation", async () => {
    const plan = await api.prepareChatAction(fixtureRef("conversation", "-1001"), "delete_group", fixtureContext);
    await expect(api.execute(plan, true, "Wrong title")).rejects.toThrow("exact chat title");
    const job = await api.execute(plan, true, "Design Team");
    expect(job.status).toBe("completed");
    expect(job.dirtyRefs).toEqual([fixtureRef("conversation", "-1001")]);
    expect(await api.refreshChats(job.dirtyRefs, fixtureContext)).toEqual([]);
    expect((await api.snapshot(fixtureContext)).chats.some((chat) => chat.id === fixtureChatId("-1001"))).toBe(false);
  });

  it("classifies likely-spam and empty conversations without conflating them", async () => {
    const snapshot = await api.snapshot(fixtureContext);
    expect(snapshot.chats.find((chat) => chat.id === fixtureChatId("303"))?.conversationState).toBe("never_replied");
    expect(snapshot.chats.find((chat) => chat.id === fixtureChatId("304"))?.conversationState).toBe("empty");
    expect(snapshot.chats.find((chat) => chat.id === fixtureChatId("101"))?.conversationState).toBe("active");
  });

  it("removes a chat from the list after whole-history revocation", async () => {
    const plan = await api.prepareChatAction(fixtureRef("conversation", "303"), "clear_history", fixtureContext);
    await api.execute(plan, true, "Prize Support");
    expect((await api.snapshot(fixtureContext)).chats.some((chat) => chat.id === fixtureChatId("303"))).toBe(false);
  });

  it("removes an empty DM only for this account when full revocation is unavailable", async () => {
    const chat = (await api.snapshot(fixtureContext)).chats.find((candidate) => candidate.id === fixtureChatId("304"));
    expect(chat?.conversationState).toBe("empty");
    expect(chat?.capabilities.canClearForEveryone).toBe(false);
    expect(chat?.capabilities.canRemoveForSelf).toBe(true);

    const plan = await api.prepareChatAction(fixtureRef("conversation", "304"), "remove_chat_for_self", fixtureContext);
    expect(plan.confirmationTier).toBe("medium");
    const job = await api.execute(plan, true, null);

    expect(job.status).toBe("completed");
    expect((await api.snapshot(fixtureContext)).chats.some((candidate) => candidate.id === fixtureChatId("304"))).toBe(false);
  });

  it("enumerates every message an admin may delete before leaving", async () => {
    const chat = (await api.snapshot(fixtureContext)).chats.find((candidate) => candidate.id === fixtureChatId("-1003"));
    expect(chat?.capabilities.canLeaveChat).toBe(true);
    expect(chat?.capabilities.canClearForEveryone).toBe(false);
    expect(chat?.capabilities.canDeleteOthers).toBe(true);

    const plan = await api.prepareChatAction(fixtureRef("conversation", "-1003"), "leave_chat", fixtureContext);
    expect(plan.operation).toBe("delete_all_messages_and_leave");
    expect(plan.confirmationTier).toBe("high");
    expect(plan.summary).toMatchObject({ selected: 3, deleteForEveryone: 1, cannotDelete: 2 });
    const job = await api.execute(plan, true, "Volunteer Archive");
    expect(job.status).toBe("completed");
    expect(job.deleted).toBe(1);
    expect(job.skipped).toBe(2);
    expect((await api.snapshot(fixtureContext)).chats.some((candidate) => candidate.id === fixtureChatId("-1003"))).toBe(false);
  });

  it("uses Telegram's complete-history cleanup before an admin leaves", async () => {
    const plan = await api.prepareChatAction(fixtureRef("conversation", "-1002"), "leave_chat", fixtureContext);
    expect(plan.operation).toBe("clear_history_and_leave");
    expect(plan.confirmationTier).toBe("high");

    const job = await api.execute(plan, true, "Neighborhood Exchange");
    expect(job.status).toBe("completed");
    expect((await api.snapshot(fixtureContext)).chats.some((candidate) => candidate.id === fixtureChatId("-1002"))).toBe(false);
  });

  it("still permits leaving when the account has no messages to revoke", async () => {
    const plan = await api.prepareChatAction(fixtureRef("conversation", "-1004"), "leave_chat", fixtureContext);
    expect(plan.confirmationTier).toBe("medium");
    expect(plan.summary).toEqual({
      selected: 0,
      deleteForEveryone: 0,
      selfOnly: 0,
      cannotDelete: 0
    });

    const job = await api.execute(plan, true, null);
    expect(job.status).toBe("completed");
    expect(job.deleted).toBe(0);
    expect((await api.snapshot(fixtureContext)).chats.some((candidate) => candidate.id === fixtureChatId("-1004"))).toBe(false);
  });

  it("deletes only my group messages without leaving or changing other messages", async () => {
    const plan = await api.prepareOwnMessages(fixtureRef("conversation", "-1003"), fixtureContext);
    expect(plan.operation).toBe("delete_my_messages");
    expect(plan.summary).toMatchObject({ selected: 1, deleteForEveryone: 1 });

    const job = await api.execute(plan, true, "Volunteer Archive");
    expect(job.status).toBe("completed");
    expect(job.dirtyRefs).toEqual([fixtureRef("conversation", "-1003")]);

    const remaining = await api.search({
      query: "",
      conversations: [fixtureRef("conversation", "-1003")],
      chatKinds: [],
      contentKinds: [],
      direction: "any",
      excludePinned: false,
      limit: 100
    }, fixtureContext);
    expect(remaining.messages.some((message) => message.isOutgoing)).toBe(false);
    expect(remaining.messages.map((message) => message.messageId)).toEqual(expect.arrayContaining([fixtureMessageId("-1003", "32"), fixtureMessageId("-1003", "33")]));
    expect((await api.refreshChats([fixtureRef("conversation", "-1003")], fixtureContext)).map((chat) => chat.id)).toEqual([fixtureChatId("-1003")]);
  });

  it("exposes production-shaped browser connection defaults without secrets", async () => {
    const settings = await api.connectionSettings(fixtureContext);
    expect(settings).not.toHaveProperty("runtimeMode");
    expect(settings.apiHashConfigured).toBe(false);
    expect(settings).not.toHaveProperty("apiHash");
  });
});
