import { beforeEach, describe, expect, it } from "vitest";
import { api } from "./api";
import { hasCryptoWallet } from "./demo";

describe("browser demo API", () => {
  beforeEach(async () => {
    await api.resetDemo();
  });

  it("searches captions and message text without a cloud index", async () => {
    const response = await api.search({
      query: "passport apartment",
      chatIds: [],
      chatKinds: [],
      contentKinds: [],
      direction: "any",
      excludePinned: false,
      limit: 100
    });
    expect(response.messages).toHaveLength(1);
    expect(response.messages[0].contentKind).toBe("photo");
  });

  it("searches a keyword across every chat when no chat scope is selected", async () => {
    const response = await api.search({
      query: "recovery phrase",
      chatIds: [],
      chatKinds: [],
      contentKinds: [],
      direction: "any",
      excludePinned: false,
      limit: 100
    });
    expect(response.messages).toHaveLength(1);
    expect(response.messages[0].chatId).toBe(202);
  });

  it("finds multiple sensitive-data categories without a keyword", async () => {
    const response = await api.search({
      query: "",
      chatIds: [],
      chatKinds: [],
      contentKinds: [],
      direction: "any",
      excludePinned: false,
      privacyScan: true,
      limit: 100
    });
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
      { chatId: -1001, messageId: 14 },
      { chatId: -1003, messageId: 32 }
    ]);
    expect(plan.summary).toMatchObject({
      selected: 2,
      deleteForEveryone: 1,
      cannotDelete: 1
    });

    const job = await api.execute(plan, true, null);
    expect(job.status).toBe("completed");
    expect(job.deleted).toBe(1);
    expect(job.skipped).toBe(1);
    expect(job.targetChatIds).toEqual([-1001]);

    const response = await api.search({
      query: "",
      chatIds: [-1001],
      chatKinds: [],
      contentKinds: [],
      direction: "any",
      excludePinned: false,
      limit: 100
    });
    expect(response.messages.some((message) => message.messageId === 14)).toBe(false);
  });

  it("requires the exact group title for a critical operation", async () => {
    const plan = await api.prepareChatAction(-1001, "delete_group");
    await expect(api.execute(plan, true, "Wrong title")).rejects.toThrow("exact chat title");
    const job = await api.execute(plan, true, "Design Team");
    expect(job.status).toBe("completed");
    expect(job.targetChatIds).toEqual([-1001]);
    expect(await api.refreshChats(job.targetChatIds)).toEqual([]);
    expect((await api.snapshot()).chats.some((chat) => chat.id === -1001)).toBe(false);
  });

  it("classifies likely-spam and empty conversations without conflating them", async () => {
    const snapshot = await api.snapshot();
    expect(snapshot.chats.find((chat) => chat.id === 303)?.conversationState).toBe("never_replied");
    expect(snapshot.chats.find((chat) => chat.id === 304)?.conversationState).toBe("empty");
    expect(snapshot.chats.find((chat) => chat.id === 101)?.conversationState).toBe("active");
  });

  it("removes a chat from the list after whole-history revocation", async () => {
    const plan = await api.prepareChatAction(303, "clear_history");
    await api.execute(plan, true, "Prize Support");
    expect((await api.snapshot()).chats.some((chat) => chat.id === 303)).toBe(false);
  });

  it("removes an empty DM only for this account when full revocation is unavailable", async () => {
    const chat = (await api.snapshot()).chats.find((candidate) => candidate.id === 304);
    expect(chat?.conversationState).toBe("empty");
    expect(chat?.capabilities.canClearForEveryone).toBe(false);
    expect(chat?.capabilities.canRemoveForSelf).toBe(true);

    const plan = await api.prepareChatAction(304, "remove_chat_for_self");
    expect(plan.confirmationTier).toBe("medium");
    const job = await api.execute(plan, true, null);

    expect(job.status).toBe("completed");
    expect((await api.snapshot()).chats.some((candidate) => candidate.id === 304)).toBe(false);
  });

  it("enumerates every message an admin may delete before leaving", async () => {
    const chat = (await api.snapshot()).chats.find((candidate) => candidate.id === -1003);
    expect(chat?.capabilities.canLeaveChat).toBe(true);
    expect(chat?.capabilities.canClearForEveryone).toBe(false);
    expect(chat?.capabilities.canDeleteOthers).toBe(true);

    const plan = await api.prepareChatAction(-1003, "leave_chat");
    expect(plan.operation).toBe("delete_all_messages_and_leave");
    expect(plan.confirmationTier).toBe("high");
    expect(plan.summary).toMatchObject({ selected: 3, deleteForEveryone: 1, cannotDelete: 2 });
    const job = await api.execute(plan, true, "Volunteer Archive");
    expect(job.status).toBe("completed");
    expect(job.deleted).toBe(1);
    expect(job.skipped).toBe(2);
    expect((await api.snapshot()).chats.some((candidate) => candidate.id === -1003)).toBe(false);
  });

  it("uses Telegram's complete-history cleanup before an admin leaves", async () => {
    const plan = await api.prepareChatAction(-1002, "leave_chat");
    expect(plan.operation).toBe("clear_history_and_leave");
    expect(plan.confirmationTier).toBe("high");

    const job = await api.execute(plan, true, "Neighborhood Exchange");
    expect(job.status).toBe("completed");
    expect((await api.snapshot()).chats.some((candidate) => candidate.id === -1002)).toBe(false);
  });

  it("still permits leaving when the account has no messages to revoke", async () => {
    const plan = await api.prepareChatAction(-1004, "leave_chat");
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
    expect((await api.snapshot()).chats.some((candidate) => candidate.id === -1004)).toBe(false);
  });

  it("deletes only my group messages without leaving or changing other messages", async () => {
    const plan = await api.prepareOwnMessages(-1003);
    expect(plan.operation).toBe("delete_my_messages");
    expect(plan.summary).toMatchObject({ selected: 1, deleteForEveryone: 1 });

    const job = await api.execute(plan, true, "Volunteer Archive");
    expect(job.status).toBe("completed");
    expect(job.targetChatIds).toEqual([-1003]);

    const remaining = await api.search({
      query: "",
      chatIds: [-1003],
      chatKinds: [],
      contentKinds: [],
      direction: "any",
      excludePinned: false,
      limit: 100
    });
    expect(remaining.messages.some((message) => message.isOutgoing)).toBe(false);
    expect(remaining.messages.map((message) => message.messageId)).toEqual(expect.arrayContaining([32, 33]));
    expect((await api.refreshChats([-1003])).map((chat) => chat.id)).toEqual([-1003]);
  });

  it("exposes production-shaped browser connection defaults without secrets", async () => {
    const settings = await api.connectionSettings();
    expect(settings).not.toHaveProperty("runtimeMode");
    expect(settings.apiHashConfigured).toBe(false);
    expect(settings).not.toHaveProperty("apiHash");
  });
});
