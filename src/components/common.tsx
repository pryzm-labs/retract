import {
  Contact,
  FileText,
  FileType2,
  Image,
  MapPin,
  MessageSquareText,
  Mic2,
  Music2,
  PlaySquare,
  Shield,
  ShieldCheck,
  Sparkles,
  Sticker,
  UserRound,
  Vote
} from "lucide-react";
import type { ChatRole, ContentKind } from "../types";

const avatarPalette = [
  ["#48657d", "#a6c4d8"],
  ["#6c557e", "#d3b9e0"],
  ["#496d62", "#b4d8ca"],
  ["#7b6045", "#dfc5a7"],
  ["#635e84", "#c8c1e9"],
  ["#526d83", "#b3d0e6"]
];

export function Avatar({ name, seed, size = 36 }: { name: string; seed: number; size?: number }) {
  const palette = avatarPalette[seed % avatarPalette.length];
  const initials = name
    .split(/\s+/)
    .slice(0, 2)
    .map((part) => part[0])
    .join("")
    .toUpperCase();
  return (
    <span
      className="avatar"
      aria-hidden="true"
      style={{
        width: size,
        height: size,
        background: `linear-gradient(145deg, ${palette[1]}, ${palette[0]})`,
        fontSize: Math.max(10, size * 0.32)
      }}
    >
      {initials}
    </span>
  );
}

export function RoleIcon({ role, size = 13 }: { role: ChatRole; size?: number }) {
  if (role === "owner") return <Sparkles size={size} aria-hidden="true" />;
  if (role === "admin_with_delete") return <ShieldCheck size={size} aria-hidden="true" />;
  if (role === "admin_limited") return <Shield size={size} aria-hidden="true" />;
  return <UserRound size={size} aria-hidden="true" />;
}

export function roleLabel(role: ChatRole): string {
  return {
    owner: "Owner",
    admin_with_delete: "Admin · can delete",
    admin_limited: "Admin · limited",
    member: "Member"
  }[role];
}

export function ContentIcon({ kind, size = 15 }: { kind: ContentKind; size?: number }) {
  const props = { size, strokeWidth: 1.8, "aria-hidden": true as const };
  switch (kind) {
    case "photo": return <Image {...props} />;
    case "video": return <PlaySquare {...props} />;
    case "file": return <FileType2 {...props} />;
    case "voice": return <Mic2 {...props} />;
    case "audio": return <Music2 {...props} />;
    case "animation": return <PlaySquare {...props} />;
    case "sticker": return <Sticker {...props} />;
    case "poll": return <Vote {...props} />;
    case "location": return <MapPin {...props} />;
    case "contact": return <Contact {...props} />;
    case "service": return <FileText {...props} />;
    case "text": return <MessageSquareText {...props} />;
    default: return <FileText {...props} />;
  }
}

export function contentLabel(kind: ContentKind): string {
  return kind === "animation"
    ? "GIF"
    : kind.replace("_", " ").replace(/^./, (letter) => letter.toUpperCase());
}

export function formatCompactDate(value: string): string {
  const date = new Date(value);
  const now = new Date(Date.UTC(2026, 7, 16));
  const sameYear = date.getUTCFullYear() === now.getUTCFullYear();
  return new Intl.DateTimeFormat(undefined, {
    month: "short",
    day: "numeric",
    ...(sameYear ? {} : { year: "numeric" }),
    hour: "numeric",
    minute: "2-digit"
  }).format(date);
}

export function plural(count: number, singular: string, pluralForm = `${singular}s`): string {
  return `${count.toLocaleString()} ${count === 1 ? singular : pluralForm}`;
}

