import { create } from "zustand";
import type { Target } from "@/api/sessions";

export function targetKey({ workspace, session, agent }: Target) {
  return JSON.stringify([workspace, session, agent]);
}

export const useDrafts = create<{
  drafts: Record<string, string>;
  change: (key: string, value: string) => void;
}>((set) => ({
  drafts: {},
  change: (key, value) => {
    set(({ drafts }) => ({ drafts: { ...drafts, [key]: value } }));
  },
}));
