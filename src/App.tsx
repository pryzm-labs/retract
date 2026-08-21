import { AlertTriangle, CheckCircle2, LoaderCircle, X } from "lucide-react";
import { useCallback, useDeferredValue, useEffect, useMemo, useRef, useState } from "react";
import { api } from "@retract/api";
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
  const [selectedChatId, setSelectedChatId] = useState<number | null>(null);
  const [chatQuery, setChatQuery] = useState("");
  const [selectedMessages, setSelectedMessages] = useState<Map<string, MessageSnapshot>>(new Map());
  const [plan, setPlan] = useState<PlanView | null>(null);
  const [loading, setLoading] = useState(true);
  const [syncingCatalog, setSyncingCatalog] = useState(false);
  const [catalogProgress, setCatalogProgress] = useState<CatalogProgress>({ phase: "idle", total: 0, processed: 0 });
  const [refreshingCatalog, setRefreshingCatalog] = useState(false);
  const [settlingRemovalChatIds, setSettlingRemovalChatIds] = useState<Set<number>>(new Set());
  const [searching, setSearching] = useState(false);
  const [busyLabel, setBusyLabel] = useState<string | null>(null);
  const [searchVersion, setSearchVersion] = useState(0);
  const [toast, setToast] = useState<ToastState | null>(null);
  const previousHadActiveJobs = useRef(false);
  const authRefreshInFlight = useRef(false);
  const jobsRefreshInFlight = useRef(false);
  const actionInFlight = useRef(false);
  const actionGeneration = useRef(0);
  const catalogSyncStarted = useRef(false);
  const snapshotLoadGeneration = useRef(0);
  const backgroundRefreshGeneration = useRef(0);

  const loadSnapshot = useCallback(async (failureContext?: string) => {
    const generation = ++snapshotLoadGeneration.current;
    try {
      const next = await api.snapshot();
      if (generation !== snapshotLoadGeneration.current) return null;
      setSnapshot(next);
      setJobs(next.recentJobs);
      setCatalogProgress({ phase: "ready", total: next.chats.length, processed: next.chats.length });
      catalogSyncStarted.current = next.auth.stage === "ready";
      return next;
    } catch (error) {
      if (generation === snapshotLoadGeneration.current) {
        showError(error, setToast, failureContext);
      }
      return null;
    }
  }, []);

  const refreshAffectedChatsInBackground = useCallback((chatIds: number[], removedChatIds: number[] = []) => {
    const uniqueChatIds = Array.from(new Set(chatIds));
    const removed = new Set(removedChatIds);
    if (removed.size > 0) {
      setSnapshot((current) => current
        ? { ...current, chats: current.chats.filter((chat) => !removed.has(chat.id)) }
        : current);
      setSelectedChatId((current) => current !== null && removed.has(current) ? null : current);
      setSelectedMessages((current) => new Map(
        Array.from(current.entries()).filter(([, message]) => !removed.has(message.chatId))
      ));
      setSettlingRemovalChatIds((current) => {
        const next = new Set(current);
        removed.forEach((chatId) => next.delete(chatId));
        return next;
      });
    }
    if (uniqueChatIds.length === 0) {
      setSearchVersion((value) => value + 1);
      return;
    }
    const generation = ++backgroundRefreshGeneration.current;
    setRefreshingCatalog(true);
    void api.refreshChats(uniqueChatIds)
      .then((refreshedChats) => {
        if (generation !== backgroundRefreshGeneration.current) return;
        const requested = new Set(uniqueChatIds);
        const applicableChats = refreshedChats.filter((chat) => !removed.has(chat.id));
        const returned = new Set(applicableChats.map((chat) => chat.id));
        setSnapshot((current) => {
          if (!current) return current;
          const chats = current.chats
            .filter((chat) => !requested.has(chat.id))
            .concat(applicableChats)
            .sort((left, right) => left.title.localeCompare(right.title));
          return { ...current, chats };
        });
        setSelectedChatId((current) => current !== null && requested.has(current) && !returned.has(current) ? null : current);
        setSearchVersion((value) => value + 1);
      })
      .catch((error) => {
        if (generation === backgroundRefreshGeneration.current) {
          showError(error, setToast, "Cleanup finished, but Retract could not refresh the affected chats");
        }
      })
      .finally(() => {
        if (generation === backgroundRefreshGeneration.current) {
          setRefreshingCatalog(false);
        }
      });
  }, []);

  const loadBootstrapSnapshot = useCallback(async () => {
    try {
      const next = await api.bootstrapSnapshot();
      setSnapshot(next);
      setJobs(next.recentJobs);
      setCatalogProgress(next.runtimeMode === "demo"
        ? { phase: "ready", total: next.chats.length, processed: next.chats.length }
        : { phase: "idle", total: 0, processed: 0 });
      catalogSyncStarted.current = false;
      return next;
    } catch (error) {
      showError(error, setToast);
      return null;
    }
  }, []);

  const refreshAuth = useCallback(async () => {
    if (authRefreshInFlight.current) return;
    authRefreshInFlight.current = true;
    try {
      const auth = await api.authSnapshot();
      setSnapshot((current) => current ? { ...current, auth } : current);
      if (auth.stage === "ready") {
        if (!catalogSyncStarted.current) {
          catalogSyncStarted.current = true;
          setSyncingCatalog(true);
          void loadSnapshot().finally(() => setSyncingCatalog(false));
        }
      } else {
        catalogSyncStarted.current = false;
      }
    } catch (error) {
      showError(error, setToast);
    } finally {
      authRefreshInFlight.current = false;
    }
  }, [loadSnapshot]);

  const loadConnectionSettings = useCallback(async () => {
    try {
      const next = await api.connectionSettings();
      setConnectionSettings(next);
      if (!next.setupComplete) setSettingsOpen(true);
    } catch (error) {
      showError(error, setToast);
    }
  }, []);

  useEffect(() => {
    let disposed = false;
    void Promise.all([loadBootstrapSnapshot(), loadConnectionSettings()]).then(([initial]) => {
      if (disposed) return;
      setLoading(false);
      if (initial?.runtimeMode === "live" && initial.auth.stage === "ready") {
        catalogSyncStarted.current = true;
        setSyncingCatalog(true);
        void loadSnapshot().finally(() => setSyncingCatalog(false));
      }
    });
    return () => { disposed = true; };
  }, [loadBootstrapSnapshot, loadSnapshot, loadConnectionSettings]);

  useEffect(() => {
    if (!snapshot || snapshot.runtimeMode !== "live" || snapshot.auth.stage === "ready") return;
    const interval = window.setInterval(() => { void refreshAuth(); }, 750);
    return () => window.clearInterval(interval);
  }, [snapshot?.runtimeMode, snapshot?.auth.stage, refreshAuth]);

  useEffect(() => {
    if (!syncingCatalog) return;
    let disposed = false;
    const refreshProgress = async () => {
      try {
        const progress = await api.catalogProgress();
        if (!disposed) setCatalogProgress(progress);
      } catch {
        // Catalog loading itself owns user-facing errors. Progress polling is
        // best-effort and must never obscure or interrupt the real operation.
      }
    };
    void refreshProgress();
    const interval = window.setInterval(() => { void refreshProgress(); }, 300);
    return () => {
      disposed = true;
      window.clearInterval(interval);
    };
  }, [syncingCatalog]);

  const chats = snapshot?.chats || [];
  const activeChat = chats.find((chat) => chat.id === selectedChatId);

  const scopedChatIds = useMemo(() => {
    if (selectedChatId !== null) return [selectedChatId];
    if (scope === "admin") {
      return chats
        .filter((chat) => chat.capabilities.role !== "member")
        .map((chat) => chat.id);
    }
    if (scope === "unanswered") return chats.filter((chat) => chat.conversationState === "never_replied").map((chat) => chat.id);
    if (scope === "empty") return chats.filter((chat) => chat.conversationState === "empty").map((chat) => chat.id);
    if (scope === "archive") return chats.filter((chat) => chat.archived).map((chat) => chat.id);
    return [];
  }, [chats, scope, selectedChatId]);

  const dateBounds = useMemo(() => searchDateBounds(dateFilter), [dateFilter]);

  useEffect(() => {
    if (!snapshot) return;
    let disposed = false;
    const timeout = window.setTimeout(async () => {
      setSearching(true);
      try {
        const response = await api.search({
          query: deferredQuery,
          chatIds: scopedChatIds,
          chatKinds: [],
          contentKinds: contentKindsForFilter[contentFilter],
          direction,
          minDate: dateBounds.minDate,
          maxDate: dateBounds.maxDate,
          excludePinned,
          privacyScan,
          limit: 500
        });
        if (!disposed) setResults(response);
      } catch (error) {
        if (!disposed) showError(error, setToast);
      } finally {
        if (!disposed) setSearching(false);
      }
    }, 120);
    return () => {
      disposed = true;
      window.clearTimeout(timeout);
    };
  }, [snapshot, deferredQuery, scopedChatIds, contentFilter, direction, dateBounds, excludePinned, privacyScan, searchVersion]);

  const hasActiveJobs = jobs.some((job) => job.status === "queued" || job.status === "running");
  const pendingRemovalChatIds = useMemo(() => {
    const pending = new Set(settlingRemovalChatIds);
    jobs
      .filter((job) => (job.status === "queued" || job.status === "running") && operationRemovesChat(job.operation))
      .forEach((job) => job.targetChatIds.forEach((chatId) => pending.add(chatId)));
    return pending;
  }, [jobs, settlingRemovalChatIds]);
  useEffect(() => {
    if (!hasActiveJobs) return;
    previousHadActiveJobs.current = true;
    let trackedActiveJobs = new Set(
      jobs
        .filter((job) => job.status === "queued" || job.status === "running")
        .map((job) => job.id)
    );
    const affectedChatIds = new Set<number>();
    const completedRemovalChatIds = new Set<number>();
    const failedRemovalChatIds = new Set<number>();
    const interval = window.setInterval(async () => {
      if (jobsRefreshInFlight.current) return;
      jobsRefreshInFlight.current = true;
      try {
        const next = await api.jobs();
        setJobs(next);
        for (const job of next) {
          if (trackedActiveJobs.has(job.id) && job.status !== "queued" && job.status !== "running") {
            job.targetChatIds.forEach((chatId) => affectedChatIds.add(chatId));
            if (operationRemovesChat(job.operation)) {
              const destination = job.status === "completed" ? completedRemovalChatIds : failedRemovalChatIds;
              job.targetChatIds.forEach((chatId) => destination.add(chatId));
            }
          }
        }
        trackedActiveJobs = new Set(
          next
            .filter((job) => job.status === "queued" || job.status === "running")
            .map((job) => job.id)
        );
        const stillActive = next.some((job) => job.status === "queued" || job.status === "running");
        if (!stillActive && previousHadActiveJobs.current) {
          previousHadActiveJobs.current = false;
          if (failedRemovalChatIds.size > 0) {
            setSettlingRemovalChatIds((current) => {
              const nextIds = new Set(current);
              failedRemovalChatIds.forEach((chatId) => nextIds.delete(chatId));
              return nextIds;
            });
            setToast({
              tone: "error",
              message: "Telegram did not remove one or more chats. They remain available; review Job activity for the failure."
            });
          }
          refreshAffectedChatsInBackground(Array.from(affectedChatIds), Array.from(completedRemovalChatIds));
        }
      } catch (error) {
        showError(error, setToast);
      } finally {
        jobsRefreshInFlight.current = false;
      }
    }, 700);
    return () => window.clearInterval(interval);
  }, [hasActiveJobs, refreshAffectedChatsInBackground]);

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
        : results.messages.filter((candidate) => candidate.chatId === message.chatId && candidate.albumId === message.albumId);
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

  const applyConnectionSettings = (result: SaveConnectionSettingsResult) => {
    actionGeneration.current += 1;
    actionInFlight.current = false;
    setBusyLabel(null);
    snapshotLoadGeneration.current += 1;
    backgroundRefreshGeneration.current += 1;
    catalogSyncStarted.current = result.snapshot.auth.stage === "ready";
    setSyncingCatalog(false);
    setCatalogProgress({
      phase: "ready",
      total: result.snapshot.chats.length,
      processed: result.snapshot.chats.length
    });
    setRefreshingCatalog(false);
    setSettlingRemovalChatIds(new Set());
    setConnectionSettings(result.connectionSettings);
    setSnapshot(result.snapshot);
    setJobs(result.snapshot.recentJobs);
    setSettingsOpen(false);
    setSelectedChatId(null);
    setSelectedMessages(new Map());
    setPlan(null);
    setSearchVersion((value) => value + 1);
    setToast({ tone: "success", message: "Connection settings applied." });
  };

  const prepareSelected = async () => {
    const generation = beginAction("Preparing deletion review…");
    if (generation === null) return;
    try {
      const prepared = await api.prepareSelection(selected.map((message) => ({
        chatId: message.chatId,
        messageId: message.messageId
      })));
      if (generation === actionGeneration.current) setPlan(prepared);
    } catch (error) {
      if (generation === actionGeneration.current) showError(error, setToast);
    } finally {
      endAction(generation);
    }
  };

  const prepareChatAction = async (operation: PlanOperation) => {
    if (!activeChat) return;
    const generation = beginAction(operation === "leave_chat"
      ? "Determining maximum cleanup scope…"
      : "Checking current chat authority…");
    if (generation === null) return;
    try {
      const prepared = await api.prepareChatAction(activeChat.id, operation);
      if (generation === actionGeneration.current) setPlan(prepared);
    } catch (error) {
      if (generation === actionGeneration.current) showError(error, setToast);
    } finally {
      endAction(generation);
    }
  };

  const prepareOwnMessages = async () => {
    if (!activeChat) return;
    const generation = beginAction("Finding every message you sent…");
    if (generation === null) return;
    try {
      const prepared = await api.prepareOwnMessages(activeChat.id);
      if (generation === actionGeneration.current) setPlan(prepared);
    } catch (error) {
      if (generation === actionGeneration.current) showError(error, setToast);
    } finally {
      endAction(generation);
    }
  };

  const prepareSenderAction = async (sender: MessageSnapshot) => {
    if (!activeChat) return;
    const generation = beginAction("Preparing sender-wide review…");
    if (generation === null) return;
    try {
      const prepared = await api.prepareSenderAction(activeChat.id, sender.senderId, sender.senderName);
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
    let refreshAfterExecution: number[] = [];
    let removedAfterExecution: number[] = [];
    try {
      if (plan.confirmationTier === "high" || plan.confirmationTier === "critical") {
        await api.authorizePlan(plan);
      }
      const job = await api.execute(plan, acknowledged, typedTitle);
      if (job.status !== "queued" && job.status !== "running") {
        refreshAfterExecution = job.targetChatIds;
        if (job.status === "completed" && operationRemovesChat(plan.operation)) {
          removedAfterExecution = job.targetChatIds;
        }
      }
      if (generation !== actionGeneration.current) return;
      setJobs((current) => [job, ...current.filter((candidate) => candidate.id !== job.id)]);
      if ((job.status === "queued" || job.status === "running") && operationRemovesChat(plan.operation)) {
        setSettlingRemovalChatIds((current) => new Set([...current, ...job.targetChatIds]));
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

  const cancelJob = async (jobId: string) => {
    const generation = beginAction("Requesting cancellation…");
    if (generation === null) return;
    try {
      await api.cancelJob(jobId);
      const next = await api.jobs();
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

  if (loading || !snapshot) {
    return (
      <StartupLoading />
    );
  }

  if (syncingCatalog) {
    return <CatalogLoading progress={catalogProgress} />;
  }

  if (snapshot.runtimeMode === "live" && snapshot.auth.stage !== "ready") {
    return (
      <>
        <AuthGate auth={snapshot.auth} onRefresh={refreshAuth} onOpenSettings={() => setSettingsOpen(true)} />
        {connectionSettings && settingsOpen && (
          <ConnectionSettingsDialog settings={connectionSettings} required={!connectionSettings.setupComplete} onClose={() => setSettingsOpen(false)} onSaved={applyConnectionSettings} />
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
        busy={busy}
        busyLabel={busyLabel}
        chatRemovalPending={activeChat ? pendingRemovalChatIds.has(activeChat.id) : false}
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
        <ConnectionSettingsDialog settings={connectionSettings} required={!connectionSettings.setupComplete} onClose={() => setSettingsOpen(false)} onSaved={applyConnectionSettings} />
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
