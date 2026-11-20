import { create } from "zustand";
import { ApiError, errorMessage } from "@/api/client";
import { findReceipt, sendCommand } from "@/api/sessions";
import type { CommandResult, Intent } from "@/api/sessions";

const storageKey = "kuku-unconfirmed-commands-v1";

type Pending = {
  intent: Intent;
  status: "unchecked" | "sending" | "checking" | "missing" | "unknown" | "conflict";
  message?: string;
};

type Outcome = { intent: Intent; result: CommandResult };

interface CommandStore {
  pending: Pending[];
  outcome?: Outcome;
  submit: (intent: Intent, onSaved?: () => void) => Promise<CommandResult | undefined>;
  retry: (commandId: string) => Promise<CommandResult | undefined>;
  check: (commandId: string, reason?: string) => Promise<CommandResult | undefined>;
  restore: () => void;
  discard: (commandId: string) => void;
  dismissOutcome: () => void;
}

function loadPending(): Pending[] {
  try {
    const raw = sessionStorage.getItem(storageKey);
    if (!raw) return [];
    const intents = JSON.parse(raw) as Intent[];
    return intents.map((intent) => ({ intent, status: "unchecked" }));
  } catch {
    return [];
  }
}

function persist(pending: Pending[]) {
  if (pending.length === 0) sessionStorage.removeItem(storageKey);
  else sessionStorage.setItem(storageKey, JSON.stringify(pending.map(({ intent }) => intent)));
}

export const useCommands = create<CommandStore>((set, get) => {
  function update(commandId: string, status: Pending["status"], message?: string) {
    set(({ pending }) => ({
      pending: pending.map((item) =>
        item.intent.commandId === commandId ? { ...item, status, message } : item,
      ),
    }));
  }

  function find(commandId: string) {
    return get().pending.find(({ intent }) => intent.commandId === commandId);
  }

  function forget(commandId: string) {
    const pending = get().pending.filter(({ intent }) => intent.commandId !== commandId);
    persist(pending);
    set({ pending });
  }

  function complete(intent: Intent, result: CommandResult) {
    forget(intent.commandId);
    set({ outcome: { intent, result } });
    return result;
  }

  async function deliver(intent: Intent) {
    update(intent.commandId, "sending");
    try {
      return complete(intent, await sendCommand(intent));
    } catch (error) {
      return get().check(intent.commandId, errorMessage(error));
    }
  }

  return {
    pending: loadPending(),
    submit: (intent, onSaved) => {
      const pending: Pending[] = [...get().pending, { intent, status: "sending" }];
      persist(pending);
      set({ pending });
      onSaved?.();
      return deliver(intent);
    },
    retry: async (commandId) => {
      const item = find(commandId);
      if (item?.status === "missing") return deliver(item.intent);
    },
    check: async (commandId, reason) => {
      const item = find(commandId);
      if (
        !item ||
        item.status === "checking" ||
        (item.status === "sending" && reason === undefined)
      )
        return;
      update(commandId, "checking", reason);
      try {
        const receipt = await findReceipt(item.intent);
        if (!receipt) {
          update(commandId, "missing", reason ?? "The server has no receipt for this command.");
          return;
        }
        return complete(item.intent, await sendCommand(item.intent, true));
      } catch (error) {
        if (error instanceof ApiError && error.code === "command_conflict") {
          update(commandId, "conflict", error.message);
        } else if (error instanceof ApiError && error.code === "receipt_not_found") {
          update(commandId, "missing", error.message);
        } else {
          update(commandId, "unknown", errorMessage(error));
        }
      }
    },
    restore: () => {
      for (const item of get().pending) {
        if (item.status === "unchecked") void get().check(item.intent.commandId);
      }
    },
    discard: (commandId) => {
      const item = find(commandId);
      if (item?.status === "missing" || item?.status === "conflict") forget(commandId);
    },
    dismissOutcome: () => {
      set({ outcome: undefined });
    },
  };
});
