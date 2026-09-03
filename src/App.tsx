import { AlertTriangle, CheckCircle2, LoaderCircle, X } from "lucide-react";
import { useCallback, useDeferredValue, useEffect, useMemo, useRef, useState } from "react";
import { api } from "@retract/api";
import { CommittedSettingsError } from "./providers/api";
import { sameContext, sameScope, scopeKey, resourceKey, refKey, type ActiveContext, type ScopedResourceRef, type Uuid } from "./providers/identity";
import { AuthGate } from "./components/AuthGate";
import { BrandLogo } from "./components/BrandLogo";
import { ConfirmDialog } from "./components/ConfirmDialog";
import { ConnectionSettingsDialog } from "./components/ConnectionSettingsDialog";
import { ImpactPanel } from "./components/ImpactPanel";
import { messageKey, ResultsList } from "./components/ResultsList";
import { type ContentFilter, contentKindsForFilter, type DateFilter, SearchToolbar } from "./components/SearchToolbar";
import { type ChatScope, Sidebar } from "./components/Sidebar";
import type {
  AppSnapshot,
  CatalogProgress,
  ConnectionSettings,
  JobRecord,
  MessageDirection,
  MessageSnapshot,
  PlanOperation,
  PlanView,
  SaveConnectionSettingsResult,
  SearchResponse
} from "./types";

interface ToastState {
  tone: "success" | "error";
  message: string;
}

export default function App() {
  const [snapshot, setSnapshot] = useState<AppSnapshot | null>(null);
  const [connectionSettings, setConnectionSettings] = useState<ConnectionSettings | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [results, setResults] = useState<SearchResponse>({ messages: [], returned: 0, truncated: false });
  const [jobs, setJobs] = useState<JobRecord[]>([]);
  const [query, setQuery] = useState("");
  const deferredQuery = useDeferredValue(query);
  const [direction, setDirection] = useState<MessageDirection>("any");
  const [contentFilter, setContentFilter] = useState<ContentFilter>("all");
  const [dateFilter, setDateFilter] = useState<DateFilter>("any");
  const [excludePinned, setExcludePinned] = useState(false);
  const [privacyScan, setPrivacyScan] = useState(false);
  const [scope, setScope] = useState<ChatScope>("all");
  const [selectedChatId, setSelectedChatId] = useState<string | null>(null);
  const [chatQuery, setChatQuery] = useState("");
  const [selectedMessages, setSelectedMessages] = useState<Map<string, MessageSnapshot>>(new Map());
  const [plan, setPlan] = useState<PlanView | null>(null);
  const [loading, setLoading] = useState(true);
  const [syncingCatalog, setSyncingCatalog] = useState(false);
  const [catalogProgress, setCatalogProgress] = useState<CatalogProgress>({ phase: "idle", total: 0, processed: 0 });
  const [refreshingCatalog, setRefreshingCatalog] = useState(false);
  const [settlingRemovalChatIds, setSettlingRemovalChatIds] = useState<Set<string>>(new Set());
  const [searching, setSearching] = useState(false);
  const [busyLabel, setBusyLabel] = useState<string | null>(null);
  const [searchVersion, setSearchVersion] = useState(0);
  const [toast, setToast] = useState<ToastState | null>(null);
  const actionInFlight = useRef(false);
  const actionGeneration = useRef(0);
  const epoch = useRef(0);
  const currentContext = useRef<ActiveContext | null>(null);
  const backgroundRefreshGeneration = useRef(0);
  const latestConversationRefresh = useRef(new Map<string, number>());
  const pendingConversationRefreshes = useRef(new Set<number>());
  const snapshotLoadGeneration = useRef(0);
  const catalogSyncStarted = useRef(false);
  const [startupError, setStartupError] = useState<string | null>(null);
  const pendingRemovalJobs = useRef(new Map<string, ScopedResourceRef[]>());

  const publish = useCallback((next: AppSnapshot) => {
    if (!sameContext(currentContext.current, next.context)) {
      epoch.current += 1;
      actionGeneration.current += 1;
      backgroundRefreshGeneration.current += 1;
      latestConversationRefresh.current.clear();
      pendingConversationRefreshes.current.clear();
      snapshotLoadGeneration.current += 1;
      actionInFlight.current = false;
      pendingRemovalJobs.current.clear();
      setBusyLabel(null);
      setSelectedChatId(null);
      setSelectedMessages(new Map());
      setPlan(null);
      setResults({ messages: [], returned: 0, truncated: false });
      setSettlingRemovalChatIds(new Set());
      setRefreshingCatalog(false);
      setSyncingCatalog(false);
      setSearching(false);
      catalogSyncStarted.current = false;
    }
    currentContext.current = next.context;
    setSnapshot(next);
    setJobs(next.context ? next.recentJobs.filter(job => sameScope(job.scope, next.context!.scope)) : []);
    setCatalogProgress(next.catalog);
    setStartupError(null);
  }, []);

  const loadSnapshot = useCallback(async (context: ActiveContext) => {
    const generation = ++snapshotLoadGeneration.current;
    const capturedEpoch = epoch.current;
    setSyncingCatalog(true);
    try {
      const next = await api.snapshot(context);
      if (generation !== snapshotLoadGeneration.current || capturedEpoch !== epoch.current) return;
      publish(next);
      catalogSyncStarted.current = true;
    } catch (error) {
      if (capturedEpoch === epoch.current) {
        catalogSyncStarted.current = false;
        setStartupError(error instanceof Error ? error.message : "The catalog could not be loaded.");
      }
    } finally {
      if (capturedEpoch === epoch.current) setSyncingCatalog(false);
    }
  }, [publish]);

  const refreshAuth = useCallback(async (discover = false) => {
    let capturedEpoch = epoch.current;
    try {
      const next = await api.bootstrapSnapshot(discover ? null : currentContext.current);
      if (capturedEpoch !== epoch.current) return;
      const same = sameContext(currentContext.current, next.context);
      if (same && catalogSyncStarted.current) {
        setSnapshot(current => current ? { ...current, auth: next.auth, identity: next.identity, catalog: next.catalog } : next);
        setCatalogProgress(next.catalog);
      } else {
        publish(next);
      }
      capturedEpoch = epoch.current;
      if (!connectionSettings) {
        const settings = await api.connectionSettings(next.context);
        if (capturedEpoch !== epoch.current) return;
        setConnectionSettings(settings);
        if (!settings.setupComplete) setSettingsOpen(true);
      }
      if (next.context && next.identity.state === "ready" && !catalogSyncStarted.current) {
        catalogSyncStarted.current = true;
        void loadSnapshot(next.context);
      }
    } catch (error) {
      if (capturedEpoch === epoch.current) setStartupError(error instanceof Error ? error.message : "The connection could not be verified.");
    }
  }, [publish, loadSnapshot, connectionSettings]);

  useEffect(() => {
    let disposed = false;
    void (async () => {
      try {
        const next = await api.bootstrapSnapshot();
        if (disposed) return;
        publish(next);
        const capturedEpoch = epoch.current;
        const settings = await api.connectionSettings(next.context);
        if (disposed || capturedEpoch !== epoch.current) return;
        setConnectionSettings(settings);
        if (!settings.setupComplete) setSettingsOpen(true);
        if (next.context && next.identity.state === "ready") {
          catalogSyncStarted.current = true;
          void loadSnapshot(next.context);
        }
      } catch (error) {
        if (!disposed) setStartupError(error instanceof Error ? error.message : "The workspace could not be opened.");
      } finally { if (!disposed) setLoading(false); }
    })();
    return () => { disposed = true; epoch.current += 1; };
  }, [publish, loadSnapshot]);

  useEffect(() => {
    if (!snapshot || snapshot.identity.state === "failed" || (snapshot.context && !syncingCatalog && snapshot.catalog.phase === "ready")) return;
    let pending = false;
    const interval = window.setInterval(() => {
      if (pending) return;
      pending = true;
      void refreshAuth().finally(() => { pending = false; });
    }, 750);
    return () => window.clearInterval(interval);
  }, [snapshot?.context, snapshot?.identity.state, snapshot?.catalog.phase, syncingCatalog, refreshAuth]);

  const refreshAffectedChatsInBackground = useCallback((refs: ScopedResourceRef[], removedRefs: ScopedResourceRef[] = []) => {
    const context = currentContext.current;
    if (!context) return;
    const conversations = [...new Map(refs.filter(ref => sameScope(ref.scope, context.scope)).map(ref => [refKey(ref), ref])).values()];
    const removed = new Set(removedRefs.filter(ref => sameScope(ref.scope, context.scope)).map(refKey));
    const capturedEpoch = epoch.current;
    if (!conversations.length) return;
    const generation = ++backgroundRefreshGeneration.current;
    const keys = conversations.map(refKey);
    keys.forEach(key => latestConversationRefresh.current.set(key, generation));
    pendingConversationRefreshes.current.add(generation);
    setRefreshingCatalog(true);
    void api.refreshChats(conversations, context).then(refreshed => {
      if (capturedEpoch !== epoch.current) return;
      // A newer request supersedes only its own conversations, not the other
      // records in this result. Keep absent records in this accepted key set.
      const requested = new Set(keys.filter(key => latestConversationRefresh.current.get(key) === generation));
      if (!requested.size) return;
      const applicable = refreshed.filter(chat => requested.has(refKey(chat.ref)) && !removed.has(refKey(chat.ref)));
      const returned = new Set(applicable.map(chat => refKey(chat.ref)));
      setSnapshot(current => current ? { ...current, chats: current.chats.filter(chat => !requested.has(refKey(chat.ref))).concat(applicable).sort((a, b) => a.title.localeCompare(b.title)) } : current);
      setSelectedChatId(current => current && requested.has(current) && !returned.has(current) ? null : current);
      setSelectedMessages(current => new Map([...current].filter(([, message]) => {
        const key = resourceKey(message.scope, "conversation", message.chatId);
        return !requested.has(key) || returned.has(key);
      })));
      setSettlingRemovalChatIds(current => new Set([...current].filter(key => !requested.has(key))));
      setSearchVersion(value => value + 1);
    }).catch(error => {
      if (capturedEpoch === epoch.current && keys.some(key => latestConversationRefresh.current.get(key) === generation)) showError(error, setToast, "Cleanup finished, but Retract could not refresh the affected chats");
    }).finally(() => {
      if (capturedEpoch === epoch.current) {
        pendingConversationRefreshes.current.delete(generation);
        setRefreshingCatalog(pendingConversationRefreshes.current.size > 0);
      }
    });
  }, []);

  const chats = snapshot?.chats || [];
  const activeChat = chats.find((chat) => refKey(chat.ref) === selectedChatId);
  // Reconciliation replaces this ref object; adding intents to the same record
  // retains it. This is a record revision dependency, not a catalog-wide reload.
  const activeChatRef = activeChat?.ref;

  const scopedChatIds = useMemo(() => {
    if (selectedChatId !== null) return chats.filter(chat => refKey(chat.ref) === selectedChatId).map(chat => chat.ref);
    if (scope === "admin") {
      return chats
        .filter((chat) => chat.capabilities.role !== "member")
        .map((chat) => chat.ref);
    }
    if (scope === "unanswered") return chats.filter((chat) => chat.conversationState === "never_replied").map((chat) => chat.ref);
    if (scope === "empty") return chats.filter((chat) => chat.conversationState === "empty").map((chat) => chat.ref);
    if (scope === "archive") return chats.filter((chat) => chat.archived).map((chat) => chat.ref);
    return [];
  }, [chats, scope, selectedChatId]);

  const dateBounds = useMemo(() => searchDateBounds(dateFilter), [dateFilter]);

  useEffect(() => {
    if (!snapshot?.context || snapshot.identity.state !== "ready" || syncingCatalog || snapshot.catalog.phase !== "ready") return;
    const context = snapshot.context;
    const capturedEpoch = epoch.current;
    let disposed = false;
    const timeout = window.setTimeout(async () => {
      setSearching(true);
      try {
        const response = await api.search({
          query: deferredQuery,
          conversations: scopedChatIds,
          chatKinds: [],
          contentKinds: contentKindsForFilter[contentFilter],
          direction,
          minDate: dateBounds.minDate,
          maxDate: dateBounds.maxDate,
          excludePinned,
          privacyScan,
          limit: 500
        }, context);
        if (!disposed && capturedEpoch === epoch.current) setResults(response);
      } catch (error) {
        if (!disposed && capturedEpoch === epoch.current) showError(error, setToast);
      } finally {
        if (!disposed && capturedEpoch === epoch.current) setSearching(false);
      }
    }, 120);
    return () => {
      disposed = true;
      window.clearTimeout(timeout);
    };
  }, [snapshot, deferredQuery, scopedChatIds, contentFilter, direction, dateBounds, excludePinned, privacyScan, searchVersion]);

  const hasActiveJobs = jobs.some(job => job.status === "queued" || job.status === "running");
  const pendingRemovalChatIds = settlingRemovalChatIds;
  useEffect(() => {
    const context = snapshot?.context;
    if (!hasActiveJobs || !context) return;
    const capturedEpoch = epoch.current;
    let disposed = false, pending = false;
    let tracked = new Set(jobs.filter(job => job.status === "queued" || job.status === "running").map(job => job.id));
    const interval = window.setInterval(async () => {
      if (pending) return;
      pending = true;
      try {
        const next = (await api.jobs(context)).filter(job => sameScope(job.scope, context.scope));
        if (disposed || capturedEpoch !== epoch.current) return;
        setJobs(next);
        const finished = next.filter(job => tracked.has(job.id) && job.status !== "queued" && job.status !== "running");
        const removedRefs = finished.filter(job => job.status === "completed").flatMap(job => pendingRemovalJobs.current.get(job.id) ?? []);
        for (const job of finished) {
          const pendingRefs = pendingRemovalJobs.current.get(job.id) ?? [];
          if (job.status !== "completed") {
            setSettlingRemovalChatIds(current => new Set([...current].filter(key => !pendingRefs.some(ref => refKey(ref) === key))));
          }
          pendingRemovalJobs.current.delete(job.id);
        }
        if (finished.length) refreshAffectedChatsInBackground(finished.flatMap(job => job.dirtyRefs), removedRefs);
        tracked = new Set(next.filter(job => job.status === "queued" || job.status === "running").map(job => job.id));
      } catch (error) { if (!disposed && capturedEpoch === epoch.current) showError(error, setToast); }
      finally { pending = false; }
    }, 700);
    return () => { disposed = true; window.clearInterval(interval); };
  }, [hasActiveJobs, snapshot?.context, refreshAffectedChatsInBackground]);

  useEffect(() => {
    const context = snapshot?.context;
    if (!activeChatRef || !context) return;
    const capturedEpoch = epoch.current, key = refKey(activeChatRef);
    let disposed = false;
    void api.intents([activeChatRef], context).then(intents => {
      if (!disposed && capturedEpoch === epoch.current) setSnapshot(current => current ? { ...current, chats: current.chats.map(chat => chat.ref === activeChatRef && refKey(chat.ref) === key ? { ...chat, intents } : chat) } : current);
    }).catch(error => { if (!disposed && capturedEpoch === epoch.current) showError(error, setToast); });
    return () => { disposed = true; };
  }, [activeChatRef, snapshot?.context]);
  useEffect(() => {
    if (!toast) return;
    const timeout = window.setTimeout(() => setToast(null), 4500);
    return () => window.clearTimeout(timeout);
  }, [toast]);

  useEffect(() => {
    const handleShortcut = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        document.querySelector<HTMLInputElement>(".global-search input")?.focus();
      }
    };
    window.addEventListener("keydown", handleShortcut);
    return () => window.removeEventListener("keydown", handleShortcut);
  }, []);

  const selected = useMemo(() => Array.from(selectedMessages.values()), [selectedMessages]);
  const visibleResultKeys = useMemo(() => new Set(results.messages.map(messageKey)), [results.messages]);
  const hiddenSelectionCount = selected.filter((message) => !visibleResultKeys.has(messageKey(message))).length;
  const busy = busyLabel !== null;

  const beginAction = (label: string) => {
    if (actionInFlight.current) return null;
    const generation = ++actionGeneration.current;
    actionInFlight.current = true;
    setBusyLabel(label);
    return generation;
  };

  const endAction = (generation: number) => {
    if (generation !== actionGeneration.current) return;
    actionInFlight.current = false;
    setBusyLabel(null);
  };

  const toggleMessage = (message: MessageSnapshot) => {
    setSelectedMessages((current) => {
      const next = new Map(current);
      const album = message.albumId == null
        ? [message]
        : results.messages.filter((candidate) => sameScope(candidate.scope, message.scope) && candidate.chatId === message.chatId && candidate.albumId === message.albumId);
      const albumIsSelected = album.every((candidate) => next.has(messageKey(candidate)));
      for (const candidate of album) {
        const key = messageKey(candidate);
        if (albumIsSelected) next.delete(key);
        else next.set(key, candidate);
      }
      return next;
    });
  };

  const toggleAll = () => {
    setSelectedMessages((current) => {
      const next = new Map(current);
      const allCurrentSelected = results.messages.length > 0 && results.messages.every((message) => next.has(messageKey(message)));
      for (const message of results.messages) {
        const key = messageKey(message);
        if (allCurrentSelected) next.delete(key);
        else next.set(key, message);
      }
      return next;
    });
  };

  const renderedEpoch = epoch.current;
  const recoverConnection = async (cause?: unknown) => {
    if (renderedEpoch !== epoch.current) return;
    try {
      // A rejected settings operation may already have retired the connection.
      // Discover read-only; never retry the mutation or rebind a previous plan.
      const next = await api.bootstrapSnapshot().catch(error => {
        if (cause instanceof CommittedSettingsError) return cause.snapshot;
        throw error;
      });
      if (renderedEpoch !== epoch.current) return;
      publish(next);
      if (cause) showError(cause, setToast);
      if (cause instanceof CommittedSettingsError) setSettingsOpen(false);
      if (next.context && next.identity.state === "ready") {
        catalogSyncStarted.current = true;
        void loadSnapshot(next.context);
      }
    } catch (error) {
      if (renderedEpoch === epoch.current) setStartupError(error instanceof Error ? error.message : "The connection could not be verified.");
    }
  };
  const retryIdentity = async () => {
    const generation = beginAction("Retrying connection…");
    if (generation === null) return;
    try {
      await api.retryIdentity(currentContext.current);
      if (generation === actionGeneration.current) await recoverConnection();
    } catch (cause) {
      if (generation === actionGeneration.current) await recoverConnection(cause);
    } finally { endAction(generation); }
  };
  const applyConnectionSettings = (result: SaveConnectionSettingsResult) => {
    if (renderedEpoch !== epoch.current) return;
    publish(result.snapshot);
    setConnectionSettings(result.connectionSettings);
    setSettingsOpen(false);
    setToast({ tone: "success", message: "Connection settings applied." });
    if (result.snapshot.context) {
      catalogSyncStarted.current = true;
      void loadSnapshot(result.snapshot.context);
    }
  };

  const prepareSelected = async () => {
    const context = currentContext.current;
    if (!context) return;
    const generation = beginAction("Preparing deletion review…");
    if (generation === null) return;
    try {
      const prepared = await api.prepareSelection(selected.map(message => message.ref), context);
      if (generation === actionGeneration.current) setPlan(prepared);
    } catch (error) {
      if (generation === actionGeneration.current) showError(error, setToast);
    } finally {
      endAction(generation);
    }
  };

  const prepareChatAction = async (operation: PlanOperation) => {
    const context = currentContext.current;
    if (!activeChat || !context) return;
    const generation = beginAction(operation === "leave_chat"
      ? "Determining maximum cleanup scope…"
      : "Checking current chat authority…");
    if (generation === null) return;
    try {
      const prepared = await api.prepareChatAction(activeChat.ref, operation, context);
      if (generation === actionGeneration.current) setPlan(prepared);
    } catch (error) {
      if (generation === actionGeneration.current) showError(error, setToast);
    } finally {
      endAction(generation);
    }
  };

  const prepareOwnMessages = async () => {
    const context = currentContext.current;
    if (!activeChat || !context) return;
    const generation = beginAction("Finding every message you sent…");
    if (generation === null) return;
    try {
      const prepared = await api.prepareOwnMessages(activeChat.ref, context);
      if (generation === actionGeneration.current) setPlan(prepared);
    } catch (error) {
      if (generation === actionGeneration.current) showError(error, setToast);
    } finally {
      endAction(generation);
    }
  };

  const prepareSenderAction = async (sender: MessageSnapshot) => {
    if (!sender.actorRef) return;
    const context = currentContext.current;
    if (!activeChat || !context) return;
    const generation = beginAction("Preparing sender-wide review…");
    if (generation === null) return;
    try {
      const prepared = await api.prepareSenderAction(activeChat.ref, sender.actorRef!, context);
      if (generation === actionGeneration.current) setPlan(prepared);
    } catch (error) {
      if (generation === actionGeneration.current) showError(error, setToast);
    } finally {
      endAction(generation);
    }
  };

  const executePlan = async (acknowledged: boolean, typedTitle: string | null) => {
    if (!plan) return;
    const generation = beginAction("Starting Telegram cleanup…");
    if (generation === null) return;
    let refreshAfterExecution: ScopedResourceRef[] = [];
    let removedAfterExecution: ScopedResourceRef[] = [];
    try {
      await api.authorizePlan(plan);
      if (generation !== actionGeneration.current) return;
      const job = await api.execute(plan, acknowledged, typedTitle);
      if (job.status !== "queued" && job.status !== "running") {
        refreshAfterExecution = job.dirtyRefs;
        if (job.status === "completed" && operationRemovesChat(plan.operation)) {
          removedAfterExecution = job.dirtyRefs;
        }
      }
      if (generation !== actionGeneration.current) return;
      setJobs((current) => [job, ...current.filter((candidate) => candidate.id !== job.id)]);
      if ((job.status === "queued" || job.status === "running") && operationRemovesChat(plan.operation)) {
        pendingRemovalJobs.current.set(job.id, job.dirtyRefs);
        setSettlingRemovalChatIds((current) => new Set([...current, ...job.dirtyRefs.map(refKey)]));
      }
      setPlan(null);
      setSelectedMessages(new Map());
      setToast({
        tone: "success",
        message: operationLeavesChat(plan.operation)
          ? job.status === "completed"
            ? "Maximum available history cleanup completed, then you left the chat."
            : "Cleaning available history, then leaving… This chat is locked until Telegram finishes."
          : plan.operation === "remove_chat_for_self"
            ? job.status === "completed"
              ? "Chat removed from this view."
              : "Removing this chat for your account… It is locked until Telegram finishes."
          : job.status === "completed"
            ? "Deletion completed. Syncing the local view…"
            : "Deletion job started. Every batch will be capability-checked again."
      });
    } catch (error) {
      if (generation === actionGeneration.current) showError(error, setToast);
    } finally {
      endAction(generation);
      if (generation === actionGeneration.current && refreshAfterExecution.length > 0) {
        refreshAffectedChatsInBackground(refreshAfterExecution, removedAfterExecution);
      }
    }
  };

  const cancelJob = async (jobId: Uuid) => {
    const context = currentContext.current;
    if (!context) return;
    const generation = beginAction("Requesting cancellation…");
    if (generation === null) return;
    try {
      await api.cancelJob(jobId, context);
      const next = await api.jobs(context);
      if (generation === actionGeneration.current) {
        setJobs(next);
        setToast({ tone: "success", message: "Cancellation requested. The current Telegram call may finish; no later batch will start." });
      }
    } catch (error) {
      if (generation === actionGeneration.current) showError(error, setToast);
    } finally {
      endAction(generation);
    }
  };

  if (startupError) {
    return <div className="app-loading"><section className="loading-card"><h1>Workspace not ready</h1><p role="alert">{startupError}</p><button onClick={() => { setStartupError(null); void refreshAuth(true); }}>Retry</button></section></div>;
  }

  if (loading || !snapshot) {
    return (
      <StartupLoading />
    );
  }

  if (snapshot.identity.state === "failed" || (snapshot.auth.stage === "ready" && (!snapshot.context || snapshot.identity.state !== "ready"))) {
    return <div className="app-loading"><section className="loading-card"><h1>Verifying Telegram account</h1>{snapshot.identity.state === "failed" ? <><p role="alert">{snapshot.identity.diagnostic.message}</p><p>Close another Retract process if this profile is in use, then retry. For state errors, preserve the profile and its backup; review connection settings before trying again.</p><button disabled={busy} onClick={() => void retryIdentity()}>Retry verification</button></> : <LoaderCircle className="spin" />}<button disabled={busy} onClick={() => setSettingsOpen(true)}>Connection settings</button>{settingsOpen && connectionSettings && <ConnectionSettingsDialog context={snapshot.context} settings={connectionSettings} required={!connectionSettings.setupComplete} onClose={() => setSettingsOpen(false)} onSaved={applyConnectionSettings} onSaveFailed={recoverConnection} />}</section></div>;
  }

  if (syncingCatalog || (snapshot.context && snapshot.catalog.phase !== "ready")) {
    return <CatalogLoading progress={catalogProgress} />;
  }

  if (snapshot.auth.stage !== "ready") {
    return (
      <>
        <AuthGate context={snapshot.context} auth={snapshot.auth} onRefresh={refreshAuth} onOpenSettings={() => setSettingsOpen(true)} />
        {connectionSettings && settingsOpen && (
          <ConnectionSettingsDialog context={snapshot.context} key={snapshot.context ? scopeKey(snapshot.context.scope) + snapshot.context.sessionGeneration : "setup"} settings={connectionSettings} required={!connectionSettings.setupComplete} onClose={() => setSettingsOpen(false)} onSaved={applyConnectionSettings} onSaveFailed={recoverConnection} />
        )}
      </>
    );
  }

  return (
    <div className="app-shell">
      <Sidebar
        chats={chats}
        selectedChatId={selectedChatId}
        scope={scope}
        chatQuery={chatQuery}
        accountLabel={snapshot.accountLabel}
        pendingRemovalChatIds={pendingRemovalChatIds}
        onChatQueryChange={setChatQuery}
        onSelectChat={setSelectedChatId}
        onScopeChange={setScope}
        onOpenSettings={() => setSettingsOpen(true)}
      />

      <main className="main-column">
        <SearchToolbar
          query={query}
          direction={direction}
          contentFilter={contentFilter}
          dateFilter={dateFilter}
          excludePinned={excludePinned}
          privacyScan={privacyScan}
          contextTitle={activeChat?.title}
          onQueryChange={setQuery}
          onDirectionChange={setDirection}
          onContentFilterChange={setContentFilter}
          onDateFilterChange={setDateFilter}
          onExcludePinnedChange={setExcludePinned}
          onPrivacyScanChange={setPrivacyScan}
        />
        <ResultsList
          messages={results.messages}
          chats={chats}
          selectedKeys={new Set(selectedMessages.keys())}
          loading={searching}
          refreshing={refreshingCatalog}
          query={query}
          privacyScan={privacyScan}
          truncated={results.truncated}
          onToggle={toggleMessage}
          onToggleAll={toggleAll}
        />
      </main>

      <ImpactPanel
        selected={selected}
        activeChat={activeChat}
        jobs={jobs}
        legacyHistory={snapshot.legacyHistory}
        busy={busy}
        busyLabel={busyLabel}
        chatRemovalPending={activeChat ? pendingRemovalChatIds.has(refKey(activeChat.ref)) : false}
        hiddenSelectionCount={hiddenSelectionCount}
        onReview={prepareSelected}
        onChatAction={prepareChatAction}
        onOwnMessagesAction={prepareOwnMessages}
        onSenderAction={prepareSenderAction}
        onClearSelection={() => setSelectedMessages(new Map())}
        onCancelJob={cancelJob}
      />

      {plan && (
        <ConfirmDialog plan={plan} busy={busy} onClose={() => !busy && setPlan(null)} onConfirm={executePlan} />
      )}

      {connectionSettings && settingsOpen && (
        <ConnectionSettingsDialog context={snapshot.context} key={snapshot.context ? scopeKey(snapshot.context.scope) + snapshot.context.sessionGeneration : "setup"} settings={connectionSettings} required={!connectionSettings.setupComplete} onClose={() => setSettingsOpen(false)} onSaved={applyConnectionSettings} onSaveFailed={recoverConnection} />
      )}

      {toast && (
        <div className={`toast toast-${toast.tone}`} role={toast.tone === "error" ? "alert" : "status"}>
          {toast.tone === "success" ? <CheckCircle2 size={17} /> : <AlertTriangle size={17} />}
          <span>{toast.message}</span>
          <button type="button" onClick={() => setToast(null)} aria-label="Dismiss notification"><X size={15} /></button>
        </div>
      )}
    </div>
  );
}

function StartupLoading() {
  return (
    <div className="app-loading" role="status">
      <div className="loading-card compact">
        <div className="loading-brand"><BrandLogo /><strong>Retract</strong></div>
        <LoaderCircle className="spin loading-spinner" size={22} />
        <p>Opening your workspace…</p>
      </div>
    </div>
  );
}

function CatalogLoading({ progress }: { progress: CatalogProgress }) {
  const hasTotal = progress.total > 0;
  const percentage = hasTotal
    ? Math.min(100, Math.round((progress.processed / progress.total) * 100))
    : 0;
  const discovering = progress.phase === "idle" || progress.phase === "discovering";
  return (
    <div className="app-loading" role="status" aria-live="polite">
      <section className="loading-card">
        <div className="loading-brand"><BrandLogo /><strong>Retract</strong></div>
        <p className="eyebrow">TELEGRAM CONNECTED</p>
        <h1>{discovering ? "Finding your chats" : "Preparing your workspace"}</h1>
        <p className="loading-lead">
          {discovering
            ? "Reading the main and archived Telegram chat lists…"
            : "Loading permissions and classifying empty or unanswered conversations…"}
        </p>
        <div
          className={`catalog-progress-track ${hasTotal ? "" : "is-indeterminate"}`}
          role="progressbar"
          aria-label="Chat loading progress"
          aria-valuemin={0}
          aria-valuemax={hasTotal ? progress.total : undefined}
          aria-valuenow={hasTotal ? progress.processed : undefined}
        >
          <span style={hasTotal ? { width: `${percentage}%` } : undefined} />
        </div>
        <div className="catalog-progress-copy">
          <span><LoaderCircle className="spin" size={14} />{hasTotal ? `${progress.processed.toLocaleString()} of ${progress.total.toLocaleString()} chats processed` : "Discovering chats…"}</span>
          {hasTotal && <strong>{percentage}%</strong>}
        </div>
        <small>Retract will open when search, authority, and cleanup filters are fully ready.</small>
      </section>
    </div>
  );
}

function showError(error: unknown, setter: (toast: ToastState) => void, context?: string) {
  const detail = error instanceof Error ? error.message : "An unexpected local error occurred.";
  setter({
    tone: "error",
    message: context ? `${context}: ${detail}` : detail
  });
}

function operationRemovesChat(operation: PlanOperation): boolean {
  return operation === "clear_history"
    || operation === "remove_chat_for_self"
    || operation === "clear_history_and_leave"
    || operation === "delete_all_messages_and_leave"
    || operation === "leave_chat"
    || operation === "delete_group";
}

function operationLeavesChat(operation: PlanOperation): boolean {
  return operation === "clear_history_and_leave"
    || operation === "delete_all_messages_and_leave"
    || operation === "leave_chat";
}

function searchDateBounds(filter: DateFilter): { minDate: string | null; maxDate: string | null } {
  if (filter === "any") return { minDate: null, maxDate: null };
  const now = new Date();
  if (filter === "last_30d") {
    const minimum = new Date(now);
    minimum.setUTCDate(minimum.getUTCDate() - 30);
    return { minDate: minimum.toISOString(), maxDate: now.toISOString() };
  }
  const maximum = new Date(now);
  maximum.setUTCFullYear(maximum.getUTCFullYear() - (filter === "older_2y" ? 2 : 1));
  return { minDate: null, maxDate: maximum.toISOString() };
}
