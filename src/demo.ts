import type {
  AppSnapshot,
  CatalogProgress,
  ChatSummary,
  ConversationState,
  JobRecord,
  MessageSnapshot,
  PlanOperation,
  PlanView,
  SearchRequest,
  SearchResponse,
  SensitiveDataKind
} from "./types";

const initialChats: ChatSummary[] = [
  {
    id: 101,
    title: "Maya Chen",
    kind: "direct",
    archived: false,
    memberCount: null,
    conversationState: "unknown",
    avatarSeed: 2,
    capabilities: {
      role: "member",
      canDeleteOthers: false,
      canClearForEveryone: true,
      canRemoveForSelf: true,
      canDeleteGroup: false,
      canDeleteBySender: false,
      canLeaveChat: false
    }
  },
  {
    id: -1001,
    title: "Design Team",
    kind: "supergroup",
    archived: false,
    memberCount: 24,
    conversationState: "unknown",
    avatarSeed: 5,
    capabilities: {
      role: "owner",
      canDeleteOthers: true,
      canClearForEveryone: true,
      canRemoveForSelf: false,
      canDeleteGroup: true,
      canDeleteBySender: true,
      canLeaveChat: false
    }
  },
  {
    id: -1002,
    title: "Neighborhood Exchange",
    kind: "supergroup",
    archived: false,
    memberCount: 418,
    conversationState: "unknown",
    avatarSeed: 8,
    capabilities: {
      role: "admin_with_delete",
      canDeleteOthers: true,
      canClearForEveryone: true,
      canRemoveForSelf: false,
      canDeleteGroup: false,
      canDeleteBySender: true,
      canLeaveChat: true
    }
  },
  {
    id: -1003,
    title: "Volunteer Archive",
    kind: "supergroup",
    archived: true,
    memberCount: 82,
    conversationState: "unknown",
    avatarSeed: 11,
    capabilities: {
      role: "admin_with_delete",
      canDeleteOthers: true,
      canClearForEveryone: false,
      canRemoveForSelf: false,
      canDeleteGroup: false,
      canDeleteBySender: true,
      canLeaveChat: true
    }
  },
  {
    id: -1004,
    title: "Open Source News",
    kind: "channel",
    archived: false,
    memberCount: 3204,
    conversationState: "unknown",
    avatarSeed: 14,
    capabilities: {
      role: "member",
      canDeleteOthers: false,
      canClearForEveryone: false,
      canRemoveForSelf: false,
      canDeleteGroup: false,
      canDeleteBySender: false,
      canLeaveChat: true
    }
  },
  {
    id: 202,
    title: "Old devices",
    kind: "secret",
    archived: true,
    memberCount: null,
    conversationState: "unknown",
    avatarSeed: 17,
    capabilities: {
      role: "member",
      canDeleteOthers: false,
      canClearForEveryone: true,
      canRemoveForSelf: true,
      canDeleteGroup: false,
      canDeleteBySender: false,
      canLeaveChat: false
    }
  },
  {
    id: 303,
    title: "Prize Support",
    kind: "direct",
    archived: false,
    memberCount: null,
    conversationState: "unknown",
    avatarSeed: 20,
    capabilities: {
      role: "member",
      canDeleteOthers: false,
      canClearForEveryone: true,
      canRemoveForSelf: true,
      canDeleteGroup: false,
      canDeleteBySender: false,
      canLeaveChat: false
    }
  },
  {
    id: 304,
    title: "Empty invite",
    kind: "direct",
    archived: false,
    memberCount: null,
    conversationState: "unknown",
    avatarSeed: 23,
    capabilities: {
      role: "member",
      canDeleteOthers: false,
      canClearForEveryone: false,
      canRemoveForSelf: true,
      canDeleteGroup: false,
      canDeleteBySender: false,
      canLeaveChat: false
    }
  }
];

type Fixture = Omit<MessageSnapshot, "sentAt">;

const fixtureMessages: Fixture[] = [
  msg(101, 1, 501, "Maya", false, "text", "The temporary address was 17 Juniper Lane.", "everyone"),
  msg(101, 2, 42, "You", true, "photo", "Passport scan for the apartment application", "everyone"),
  msg(101, 3, 501, "Maya", false, "text", "I deleted the shared folder already.", "everyone"),
  msg(101, 4, 42, "You", true, "voice", "Voice message · 0:18", "everyone"),
  msg(-1001, 11, 42, "You", true, "text", "Project Cedar launch credentials moved to the vault.", "everyone", true),
  msg(-1001, 12, 712, "Nora", false, "file", "cedar_research_notes.pdf · 4.8 MB", "everyone", false, 7001),
  msg(-1001, 13, 713, "Owen", false, "photo", "Whiteboard with customer email list", "everyone", false, 7001),
  msg(-1001, 14, 42, "You", true, "text", "My old phone number ends in 0441.", "everyone"),
  msg(-1001, 15, 714, "Priya", false, "poll", "Where should we hold the offsite?", "everyone"),
  msg(-1002, 21, 818, "Unknown", false, "text", "Limited offer — contact me directly", "everyone"),
  msg(-1002, 22, 818, "Unknown", false, "photo", "Advertisement image", "everyone"),
  msg(-1002, 23, 42, "You", true, "location", "Old pickup point", "everyone"),
  msg(-1002, 24, 819, "Jo", false, "contact", "Contact card · Alex R.", "everyone"),
  msg(-1003, 31, 42, "You", true, "text", "Here is my personal email for the volunteer roster.", "everyone"),
  msg(-1003, 32, 920, "Sam", false, "file", "volunteer_roster_2022.xlsx", "none", true),
  msg(-1003, 33, 921, "Lee", false, "text", "The archive should remain read-only.", "none"),
  msg(-1004, 41, 1004, "Open Source News", false, "text", "Release notes for version 8.4", "none"),
  msg(-1004, 42, 1004, "Open Source News", false, "video", "Conference keynote · 24:10", "none"),
  msg(202, 51, 42, "You", true, "text", "Recovery phrase moved offline; delete this reminder.", "everyone"),
  msg(202, 52, 1202, "Old devices", false, "text", "This secret chat only exists on this device.", "everyone"),
  msg(303, 61, 1303, "Prize Support", false, "text", "You won a prize — reply with your account details", "everyone"),
  msg(101, 5, 42, "You", true, "text", "Backup contact person@example.com · wallet 0x52908400098527886E0F7030069857D2E4169EE7", "everyone")
];

function msg(
  chatId: number,
  messageId: number,
  senderId: number,
  senderName: string,
  isOutgoing: boolean,
  contentKind: Fixture["contentKind"],
  preview: string,
  deletionReach: Fixture["deletionReach"],
  isPinned = false,
  albumId: number | null = null
): Fixture {
  return {
    chatId,
    messageId,
    senderId,
    senderName,
    isOutgoing,
    contentKind,
    preview,
    privacyFindings: [],
    deletionReach,
    isPinned,
    albumId
  };
}

function datedMessages(): MessageSnapshot[] {
  return fixtureMessages.map((message, index) => ({
    ...message,
    sentAt: new Date(Date.UTC(2026, 7, 15 - Math.floor(index / 5), 18, index, 0)).toISOString()
  }));
}

let chats = structuredClone(initialChats);
let messages = datedMessages();
let jobs: JobRecord[] = [];
const plans = new Map<string, PlanView & { refs?: Array<[number, number]>; chatId?: number; senderId?: number }>();

const delay = (milliseconds = 80) => new Promise((resolve) => setTimeout(resolve, milliseconds));

export async function demoSnapshot(): Promise<AppSnapshot> {
  await delay();
  return {
    runtimeMode: "demo",
    accountLabel: "Private demo account",
    modeReason: "No Telegram session is connected. Destructive actions affect demo fixtures only.",
    chats: structuredClone(chats.map((chat) => ({
      ...chat,
      conversationState: conversationState(chat)
    }))),
    recentJobs: structuredClone(jobs),
    safetyNotice: "Retract never downgrades a failed ‘delete for everyone’ request to ‘delete for me’.",
    auth: { stage: "ready", hint: null, qrLink: null }
  };
}

export function demoCatalogProgress(): Promise<CatalogProgress> {
  return Promise.resolve({
    phase: "ready",
    total: chats.length,
    processed: chats.length
  });
}

export async function demoRefreshChats(chatIds: number[]): Promise<ChatSummary[]> {
  await delay(25);
  const requested = new Set(chatIds);
  return structuredClone(
    chats
      .filter((chat) => requested.has(chat.id))
      .map((chat) => ({
        ...chat,
        conversationState: conversationState(chat)
      }))
  );
}

export async function demoSearch(request: SearchRequest): Promise<SearchResponse> {
  await delay(45);
  const tokens = request.query.toLowerCase().trim().split(/\s+/).filter(Boolean);
  const kindsByChat = new Map(chats.map((chat) => [chat.id, chat.kind]));
  const filtered = messages
    .map((message) => ({
      ...message,
      privacyFindings: request.privacyScan ? detectSensitiveData(message) : []
    }))
    .filter((message) => request.chatIds.length === 0 || request.chatIds.includes(message.chatId))
    .filter((message) => request.chatKinds.length === 0 || request.chatKinds.includes(kindsByChat.get(message.chatId)!))
    .filter((message) => request.contentKinds.length === 0 || request.contentKinds.includes(message.contentKind))
    .filter((message) => request.direction === "any" || (request.direction === "mine") === message.isOutgoing)
    .filter((message) => !request.excludePinned || !message.isPinned)
    .filter((message) => !request.minDate || message.sentAt >= request.minDate)
    .filter((message) => !request.maxDate || message.sentAt <= request.maxDate)
    .filter((message) => {
      const searchable = `${message.preview} ${message.senderName}`.toLowerCase();
      return tokens.every((token) => searchable.includes(token));
    })
    .filter((message) => !request.privacyScan || message.privacyFindings.length > 0)
    .sort((a, b) => b.sentAt.localeCompare(a.sentAt));
  return {
    messages: structuredClone(filtered.slice(0, request.limit)),
    returned: Math.min(filtered.length, request.limit),
    truncated: filtered.length > request.limit
  };
}

export async function demoPrepareSelection(refs: Array<{ chatId: number; messageId: number }>): Promise<PlanView> {
  await delay();
  const selected = refs
    .map((ref) => messages.find((message) => message.chatId === ref.chatId && message.messageId === ref.messageId))
    .filter((message): message is MessageSnapshot => Boolean(message));
  if (selected.length !== refs.length) throw new Error("One or more selected messages no longer exist.");
  const everyone = selected.filter((message) => message.deletionReach === "everyone").length;
  if (everyone === 0) throw new Error("None of the selected messages can be deleted for everyone.");
  const plan: PlanView & { refs: Array<[number, number]> } = {
    id: crypto.randomUUID(),
    operation: "selected_messages",
    chatTitle: null,
    summary: {
      selected: selected.length,
      deleteForEveryone: everyone,
      selfOnly: selected.filter((message) => message.deletionReach === "self_only").length,
      cannotDelete: selected.filter((message) => message.deletionReach === "none").length
    },
    confirmationTier: selected.length <= 10 ? "low" : "medium",
    fingerprint: crypto.randomUUID().replaceAll("-", ""),
    createdAt: new Date().toISOString(),
    refs: refs.map((ref) => [ref.chatId, ref.messageId])
  };
  plans.set(plan.id, plan);
  return structuredClone(plan);
}

export async function demoPrepareOwnMessages(chatId: number): Promise<PlanView> {
  await delay();
  const chat = chats.find((candidate) => candidate.id === chatId);
  if (!chat) throw new Error("Chat no longer exists.");
  const activeGroup = (chat.kind === "basic_group" || chat.kind === "supergroup")
    && (chat.capabilities.role !== "member" || chat.capabilities.canLeaveChat);
  if (!activeGroup) throw new Error("Deleting your complete message history is available only in groups you currently belong to.");
  const ownMessages = messages.filter((message) => message.chatId === chatId && message.isOutgoing);
  if (ownMessages.length === 0) throw new Error("Telegram found no messages sent by your account in this group.");
  const everyone = ownMessages.filter((message) => message.deletionReach === "everyone").length;
  if (everyone === 0) throw new Error("None of your messages can currently be deleted for everyone.");
  const plan: PlanView & { refs: Array<[number, number]>; chatId: number } = {
    id: crypto.randomUUID(),
    operation: "delete_my_messages",
    chatTitle: chat.title,
    summary: {
      selected: ownMessages.length,
      deleteForEveryone: everyone,
      selfOnly: ownMessages.filter((message) => message.deletionReach === "self_only").length,
      cannotDelete: ownMessages.filter((message) => message.deletionReach === "none").length
    },
    confirmationTier: "high",
    fingerprint: crypto.randomUUID().replaceAll("-", ""),
    createdAt: new Date().toISOString(),
    refs: ownMessages.map((message) => [message.chatId, message.messageId]),
    chatId
  };
  plans.set(plan.id, plan);
  return structuredClone(plan);
}

export async function demoPrepareChatAction(chatId: number, operation: PlanOperation): Promise<PlanView> {
  await delay();
  const chat = chats.find((candidate) => candidate.id === chatId);
  if (!chat) throw new Error("Chat no longer exists.");
  const allowed = operation === "delete_group"
    ? chat.capabilities.canDeleteGroup
    : operation === "clear_history"
      ? chat.capabilities.canClearForEveryone
      : operation === "remove_chat_for_self"
        ? chat.capabilities.canRemoveForSelf
      : operation === "leave_chat"
        ? chat.capabilities.canLeaveChat
      : false;
  if (!allowed) throw new Error("Telegram’s current capability flags do not allow this operation.");
  const leaveOperation: PlanOperation = operation !== "leave_chat"
    ? operation
    : chat.capabilities.canClearForEveryone
      ? "clear_history_and_leave"
      : chat.capabilities.canDeleteOthers
        ? "delete_all_messages_and_leave"
        : "leave_chat";
  const cleanupMessages = leaveOperation === "delete_all_messages_and_leave"
    ? messages.filter((message) => message.chatId === chatId)
    : leaveOperation === "leave_chat"
      ? messages.filter((message) => message.chatId === chatId && message.isOutgoing)
      : [];
  const plan: PlanView & { chatId: number; refs?: Array<[number, number]> } = {
    id: crypto.randomUUID(),
    operation: leaveOperation,
    chatTitle: chat.title,
    summary: leaveOperation === "leave_chat" || leaveOperation === "delete_all_messages_and_leave"
      ? {
          selected: cleanupMessages.length,
          deleteForEveryone: cleanupMessages.filter((message) => message.deletionReach === "everyone").length,
          selfOnly: cleanupMessages.filter((message) => message.deletionReach === "self_only").length,
          cannotDelete: cleanupMessages.filter((message) => message.deletionReach === "none").length
        }
      : { selected: 0, deleteForEveryone: 0, selfOnly: 0, cannotDelete: 0 },
    confirmationTier: operation === "delete_group"
      ? "critical"
      : leaveOperation === "clear_history_and_leave" || leaveOperation === "delete_all_messages_and_leave"
        ? "high"
      : leaveOperation === "leave_chat"
        ? cleanupMessages.some((message) => message.deletionReach === "everyone") ? "high" : "medium"
        : operation === "remove_chat_for_self" ? "medium" : "high",
    fingerprint: crypto.randomUUID().replaceAll("-", ""),
    createdAt: new Date().toISOString(),
    chatId,
    refs: leaveOperation === "leave_chat" || leaveOperation === "delete_all_messages_and_leave"
      ? cleanupMessages.map((message) => [message.chatId, message.messageId])
      : undefined
  };
  plans.set(plan.id, plan);
  return structuredClone(plan);
}

export async function demoPrepareSenderAction(chatId: number, senderId: number, senderName: string): Promise<PlanView> {
  await delay();
  const chat = chats.find((candidate) => candidate.id === chatId);
  if (!chat) throw new Error("Chat no longer exists.");
  if (!chat.capabilities.canDeleteBySender) throw new Error("Telegram’s current capability flags do not allow deleting by sender.");
  if (!senderName.trim()) throw new Error("Sender name is required.");
  const plan: PlanView & { chatId: number; senderId: number } = {
    id: crypto.randomUUID(),
    operation: "delete_by_sender",
    chatTitle: chat.title,
    targetSenderName: senderName,
    summary: { selected: 0, deleteForEveryone: 0, selfOnly: 0, cannotDelete: 0 },
    confirmationTier: "high",
    fingerprint: crypto.randomUUID().replaceAll("-", ""),
    createdAt: new Date().toISOString(),
    chatId,
    senderId
  };
  plans.set(plan.id, plan);
  return structuredClone(plan);
}

export async function demoExecute(
  planId: string,
  fingerprint: string,
  acknowledged: boolean,
  typedTitle?: string | null
): Promise<JobRecord> {
  const plan = plans.get(planId);
  if (!plan || plan.fingerprint !== fingerprint || !acknowledged) throw new Error("Confirmation does not match the frozen plan.");
  if ((plan.confirmationTier === "high" || plan.confirmationTier === "critical") && typedTitle !== plan.chatTitle) {
    throw new Error("Type the exact chat title to continue.");
  }
  const now = new Date().toISOString();
  const job: JobRecord = {
    id: crypto.randomUUID(),
    planId,
    operation: plan.operation,
    targetChatIds: Array.from(new Set(
      plan.chatId !== undefined
        ? [plan.chatId]
        : (plan.refs ?? [])
            .filter(([chatId, messageId]) => messages.some((message) =>
              message.chatId === chatId
              && message.messageId === messageId
              && message.deletionReach === "everyone"
            ))
            .map(([chatId]) => chatId)
    )).sort((left, right) => left - right),
    status: "running",
    total: plan.summary.deleteForEveryone,
    deleted: 0,
    skipped: plan.summary.selfOnly + plan.summary.cannotDelete,
    failed: 0,
    nextBatch: 0,
    retryAfterSeconds: null,
    errorCodes: [],
    createdAt: now,
    updatedAt: now
  };
  jobs = [job, ...jobs];
  await delay(350);
  if ((plan.operation === "selected_messages" || plan.operation === "delete_my_messages") && plan.refs) {
    const refs = new Set(plan.refs.map(([chatId, messageId]) => `${chatId}:${messageId}`));
    const before = messages.length;
    messages = messages.filter((message) => message.deletionReach !== "everyone" || !refs.has(`${message.chatId}:${message.messageId}`));
    job.deleted = before - messages.length;
  } else if (plan.operation === "clear_history" && plan.chatId !== undefined) {
    messages = messages.filter((message) => message.chatId !== plan.chatId);
    chats = chats.filter((chat) => chat.id !== plan.chatId);
  } else if (plan.operation === "remove_chat_for_self" && plan.chatId !== undefined) {
    messages = messages.filter((message) => message.chatId !== plan.chatId);
    chats = chats.filter((chat) => chat.id !== plan.chatId);
  } else if (plan.operation === "delete_group" && plan.chatId !== undefined) {
    messages = messages.filter((message) => message.chatId !== plan.chatId);
    chats = chats.filter((chat) => chat.id !== plan.chatId);
  } else if (plan.operation === "clear_history_and_leave" && plan.chatId !== undefined) {
    messages = messages.filter((message) => message.chatId !== plan.chatId);
    chats = chats.filter((chat) => chat.id !== plan.chatId);
  } else if ((plan.operation === "leave_chat" || plan.operation === "delete_all_messages_and_leave") && plan.chatId !== undefined) {
    if (plan.refs) {
      const refs = new Set(plan.refs.map(([chatId, messageId]) => `${chatId}:${messageId}`));
      const before = messages.length;
      messages = messages.filter((message) =>
        message.deletionReach !== "everyone"
        || !refs.has(`${message.chatId}:${message.messageId}`)
      );
      job.deleted = before - messages.length;
    }
    messages = messages.filter((message) => message.chatId !== plan.chatId);
    chats = chats.filter((chat) => chat.id !== plan.chatId);
  } else if (plan.operation === "delete_by_sender" && plan.chatId !== undefined && plan.senderId !== undefined) {
    messages = messages.filter((message) => message.chatId !== plan.chatId || message.senderId !== plan.senderId);
  }
  job.status = "completed";
  job.updatedAt = new Date().toISOString();
  jobs = jobs.map((candidate) => candidate.id === job.id ? structuredClone(job) : candidate);
  return structuredClone(job);
}

export async function demoJobs(): Promise<JobRecord[]> {
  return structuredClone(jobs);
}

export async function demoReset(): Promise<AppSnapshot> {
  chats = structuredClone(initialChats);
  messages = datedMessages();
  jobs = [];
  plans.clear();
  return demoSnapshot();
}

function conversationState(chat: ChatSummary): ConversationState {
  if (chat.kind === "channel") return "unknown";
  const history = messages
    .filter((message) => message.chatId === chat.id)
    .sort((a, b) => b.sentAt.localeCompare(a.sentAt));
  if (history.length === 0) return "empty";
  if (history[0].isOutgoing) return "active";
  return history.some((message) => message.isOutgoing)
    ? "awaiting_reply"
    : "never_replied";
}

function detectSensitiveData(message: MessageSnapshot): SensitiveDataKind[] {
  const findings = new Set<SensitiveDataKind>();
  const text = message.preview;
  if (/\b[A-Z0-9._%+\-]+@[A-Z0-9.\-]+\.[A-Z]{2,63}\b/i.test(text)) findings.add("email_address");
  if (hasPhoneNumber(text)) findings.add("phone_number");
  if (/\b\d{1,6}\s+(?:[A-Z0-9.'\-]+\s+){0,5}(?:street|st|road|rd|avenue|ave|lane|ln|drive|dr|boulevard|blvd|way|court|ct|place|pl|terrace|trail|parkway|highway)\b\.?/i.test(text)) findings.add("postal_address");
  if (message.contentKind === "location" || /(?:geo:|maps\.google\.|goo\.gl\/maps|maps\.apple\.|openstreetmap\.org)/i.test(text) || /[+-]?\d{1,2}(?:\.\d+)?\s*[,;]\s*[+-]?\d{1,3}(?:\.\d+)?/.test(text)) findings.add("precise_location");
  if (/\b(?:date\s+of\s+birth|birth\s*date|dob|mother'?s\s+maiden\s+name|medical\s+record\s+(?:number|id)|patient\s+id|employee\s+id|student\s+id)\b/i.test(text)) findings.add("personal_identifier");
  if (/\b(?:passport|national\s+id|identity\s+card|driver'?s?\s+licen[cs]e|tax\s+id|social\s+security|ssn)\b/i.test(text) || /\b\d{3}-\d{2}-\d{4}\b/.test(text)) findings.add("identity_document");
  if (hasCryptoWallet(text)) findings.add("crypto_wallet");
  if (/\b(?:seed\s+phrase|recovery\s+phrase|private\s+key|api[_ -]?key|access[_ -]?token|auth(?:entication)?\s+token|password|passcode|one[- ]time\s+(?:code|password)|2fa\s+code)\b/i.test(text)) findings.add("credential_or_secret");
  if (/\b(?:\d{1,3}\.){3}\d{1,3}\b/.test(text) || /\b(?:[0-9a-f]{1,4}:){2,7}[0-9a-f]{0,4}\b/i.test(text)) findings.add("network_address");
  if (message.contentKind === "contact") findings.add("contact_card");
  if (hasPaymentCard(text) || hasIban(text)) findings.add("financial_account");
  return [...findings].sort();
}

export function hasCryptoWallet(text: string): boolean {
  if (/\b0x[0-9a-f]{40}\b/i.test(text)) return true;

  const bitcoinLegacy = [...text.matchAll(/\b[123mn][1-9A-HJ-NP-Za-km-z]{25,34}\b/g)]
    .some((match) => {
      const decoded = decodeBase58(match[0]);
      return decoded?.length === 25 && [0x00, 0x05, 0x6f, 0xc4].includes(decoded[0]);
    });
  if (bitcoinLegacy) return true;

  const bitcoinSegwit = [...text.matchAll(/\b(?:bc1|tb1)[ac-hj-np-z02-9]{6,87}\b/gi)]
    .some((match) => isValidBitcoinSegwitAddress(match[0]));
  if (bitcoinSegwit) return true;

  const hasSolanaContext = /\b(?:solana|sol\s+(?:wallet|address)|wallet(?:\s+address)?)\b/i.test(text);
  if (hasSolanaContext) {
    const solanaAddress = [...text.matchAll(/\b[1-9A-HJ-NP-Za-km-z]{32,44}\b/g)]
      .some((match) => decodeBase58(match[0])?.length === 32);
    if (solanaAddress) return true;
  }

  return /\b(?:(?:ltc1|cosmos1)[ac-hj-np-z02-9]{11,71}|[LM][a-km-zA-HJ-NP-Z1-9]{25,34}|[DT][1-9A-HJ-NP-Za-km-z]{33}|r[1-9A-HJ-NP-Za-km-z]{24,34}|addr1[a-z0-9]{20,100}|G[A-Z2-7]{55}|(?:EQ|UQ)[A-Za-z0-9_-]{46})\b/i.test(text);
}

function isValidBitcoinSegwitAddress(candidate: string): boolean {
  if (candidate.length < 8 || candidate.length > 90) return false;
  if (/[a-z]/.test(candidate) && /[A-Z]/.test(candidate)) return false;

  const normalized = candidate.toLowerCase();
  const separator = normalized.lastIndexOf("1");
  const hrp = normalized.slice(0, separator);
  if (hrp !== "bc" && hrp !== "tb") return false;

  const charset = "qpzry9x8gf2tvdw0s3jn54khce6mua7l";
  const data = [...normalized.slice(separator + 1)].map((character) => charset.indexOf(character));
  if (data.length < 7 || data.some((value) => value < 0)) return false;

  const checksum = bech32Polymod([
    ...[...hrp].map((character) => character.charCodeAt(0) >> 5),
    0,
    ...[...hrp].map((character) => character.charCodeAt(0) & 31),
    ...data
  ]);
  const witnessVersion = data[0];
  if (witnessVersion > 16) return false;
  if (witnessVersion === 0 ? checksum !== 1 : checksum !== 0x2bc830a3) return false;

  const program = convert5BitGroupsToBytes(data.slice(1, -6));
  return program !== null
    && program.length >= 2
    && program.length <= 40
    && (witnessVersion !== 0 || program.length === 20 || program.length === 32);
}

function bech32Polymod(values: number[]): number {
  const generators = [0x3b6a57b2, 0x26508e6d, 0x1ea119fa, 0x3d4233dd, 0x2a1462b3];
  let checksum = 1;
  for (const value of values) {
    const highBits = checksum >>> 25;
    checksum = (((checksum & 0x01ffffff) << 5) ^ value) >>> 0;
    generators.forEach((generator, index) => {
      if (((highBits >>> index) & 1) === 1) checksum = (checksum ^ generator) >>> 0;
    });
  }
  return checksum;
}

function convert5BitGroupsToBytes(values: number[]): number[] | null {
  let accumulator = 0;
  let bitCount = 0;
  const decoded: number[] = [];
  for (const value of values) {
    if (value > 31) return null;
    accumulator = ((accumulator << 5) | value) >>> 0;
    bitCount += 5;
    while (bitCount >= 8) {
      bitCount -= 8;
      decoded.push((accumulator >>> bitCount) & 0xff);
    }
  }
  if (bitCount >= 5 || ((accumulator << (8 - bitCount)) & 0xff) !== 0) return null;
  return decoded;
}

function decodeBase58(value: string): number[] | null {
  const alphabet = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
  const decodedLittleEndian: number[] = [];
  for (const character of value) {
    let carry = alphabet.indexOf(character);
    if (carry < 0) return null;
    for (let index = 0; index < decodedLittleEndian.length; index += 1) {
      const expanded = decodedLittleEndian[index] * 58 + carry;
      decodedLittleEndian[index] = expanded & 0xff;
      carry = Math.floor(expanded / 256);
    }
    while (carry > 0) {
      decodedLittleEndian.push(carry & 0xff);
      carry = Math.floor(carry / 256);
    }
  }
  decodedLittleEndian.push(...Array(value.match(/^1*/)?.[0].length ?? 0).fill(0));
  return decodedLittleEndian.reverse();
}

function hasPaymentCard(text: string): boolean {
  return [...text.matchAll(/\b(?:\d[ -]?){13,19}\b/g)].some((match) => {
    const digits = [...match[0]].flatMap((character) => /\d/.test(character) ? [Number(character)] : []);
    if (digits.length < 13 || digits.length > 19) return false;
    const sum = [...digits].reverse().reduce((total, digit, index) => {
      if (index % 2 === 0) return total + digit;
      const doubled = digit * 2;
      return total + (doubled > 9 ? doubled - 9 : doubled);
    }, 0);
    return sum % 10 === 0;
  });
}

function hasPhoneNumber(text: string): boolean {
  return [...text.matchAll(/(?:\+?\d[\d\s().\-]{6,}\d)/g)].some((match) => {
    const digits = [...match[0]].filter((character) => /\d/.test(character));
    const start = match.index;
    const end = start + match[0].length;
    const bounded = (start === 0 || !/[A-Z0-9]/i.test(text[start - 1]))
      && (end === text.length || !/[A-Z0-9]/i.test(text[end]));
    const dateLike = /^\s*(?:\d{4}[-/.]\d{1,2}[-/.]\d{1,2}|\d{1,2}[-/.]\d{1,2}[-/.]\d{4})\s*$/.test(match[0]);
    return bounded
      && !dateLike
      && digits.length >= 8
      && digits.length <= 15
      && !digits.every((digit) => digit === digits[0]);
  });
}

function hasIban(text: string): boolean {
  return [...text.matchAll(/\b[A-Z]{2}\d{2}(?: ?[A-Z0-9]){11,30}\b/gi)].some((match) => {
    const compact = match[0].replaceAll(" ", "").toUpperCase();
    const rearranged = `${compact.slice(4)}${compact.slice(0, 4)}`;
    let remainder = 0;
    for (const character of rearranged) {
      const value = /\d/.test(character)
        ? Number(character)
        : character.charCodeAt(0) - 55;
      remainder = /\d/.test(character)
        ? (remainder * 10 + value) % 97
        : (remainder * 100 + value) % 97;
    }
    return remainder === 1;
  });
}
