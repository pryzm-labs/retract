import { CalendarDays, Check, ChevronDown, Search, ShieldAlert, X } from "lucide-react";
import type { ContentKind, MessageDirection } from "../types";

export type ContentFilter = "all" | "text" | "media" | "files" | "voice";
export type DateFilter = "any" | "last_30d" | "older_1y" | "older_2y";

export const contentKindsForFilter: Record<ContentFilter, ContentKind[]> = {
  all: [],
  text: ["text"],
  media: ["photo", "video", "animation", "sticker"],
  files: ["file"],
  voice: ["voice", "audio"]
};

interface SearchToolbarProps {
  query: string;
  direction: MessageDirection;
  contentFilter: ContentFilter;
  dateFilter: DateFilter;
  excludePinned: boolean;
  privacyScan: boolean;
  contextTitle?: string;
  onQueryChange: (value: string) => void;
  onDirectionChange: (value: MessageDirection) => void;
  onContentFilterChange: (value: ContentFilter) => void;
  onDateFilterChange: (value: DateFilter) => void;
  onExcludePinnedChange: (value: boolean) => void;
  onPrivacyScanChange: (value: boolean) => void;
}

export function SearchToolbar({
  query,
  direction,
  contentFilter,
  dateFilter,
  excludePinned,
  privacyScan,
  contextTitle,
  onQueryChange,
  onDirectionChange,
  onContentFilterChange,
  onDateFilterChange,
  onExcludePinnedChange,
  onPrivacyScanChange
}: SearchToolbarProps) {
  return (
    <header className="search-toolbar">
      <div className="search-heading-row">
        <div>
          <p className="eyebrow">MESSAGE SEARCH</p>
          <h1>{contextTitle || "Search every chat"}</h1>
        </div>
        <button
          type="button"
          className={`privacy-scan-toggle ${privacyScan ? "is-active" : ""}`}
          aria-pressed={privacyScan}
          onClick={() => onPrivacyScanChange(!privacyScan)}
        >
          <ShieldAlert size={14} /> Privacy scan
        </button>
      </div>

      <label className="global-search">
        <Search size={19} aria-hidden="true" />
        <input
          autoFocus
          value={query}
          onChange={(event) => onQueryChange(event.target.value)}
          placeholder="Names, phrases, captions, or sensitive details"
          aria-label="Search message history"
        />
        {query && <button type="button" className="clear-search" onClick={() => onQueryChange("")} aria-label="Clear search"><X size={15} /></button>}
        <kbd>⌘ K</kbd>
      </label>

      <div className="filter-row" aria-label="Search filters">
        <div className="segmented-control" aria-label="Message sender">
          {(["any", "mine", "others"] as MessageDirection[]).map((value) => (
            <button
              type="button"
              className={direction === value ? "is-active" : ""}
              onClick={() => onDirectionChange(value)}
              key={value}
            >
              {value === "any" ? "Anyone" : value === "mine" ? "Mine" : "Others"}
            </button>
          ))}
        </div>
        <span className="filter-divider" />
        {(["all", "text", "media", "files", "voice"] as ContentFilter[]).map((filter) => (
          <button
            type="button"
            className={`filter-chip ${contentFilter === filter ? "is-active" : ""}`}
            onClick={() => onContentFilterChange(filter)}
            key={filter}
          >
            {filter[0].toUpperCase() + filter.slice(1)}
          </button>
        ))}
        <label className="date-filter">
          <span className="sr-only">Message date</span>
          <CalendarDays size={13} aria-hidden="true" />
          <select value={dateFilter} onChange={(event) => onDateFilterChange(event.target.value as DateFilter)}>
            <option value="any">Any date</option>
            <option value="last_30d">Last 30 days</option>
            <option value="older_1y">Older than 1 year</option>
            <option value="older_2y">Older than 2 years</option>
          </select>
          <ChevronDown className="date-chevron" size={12} aria-hidden="true" />
        </label>
        <label className="protect-pinned">
          <input type="checkbox" checked={excludePinned} onChange={(event) => onExcludePinnedChange(event.target.checked)} />
          <span className="protect-check" aria-hidden="true">{excludePinned && <Check size={10} />}</span>
          <span>Protect pinned</span>
        </label>
      </div>
      {privacyScan && (
        <p className="privacy-scan-note" role="status">
          Scanning message text, captions, filenames, contact cards, and Telegram location messages across the current scope. Review false positives; pixels inside photos and external copies are not inspected.
        </p>
      )}
    </header>
  );
}
