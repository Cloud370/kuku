import { useEffect, useRef, useState } from "react";
import { ApiError, errorMessage } from "@/api/client";
import { fetchHistory, fetchSnapshot, observe } from "@/api/sessions";
import type { Target } from "@/api/sessions";
import {
  applyObservation,
  applySnapshot,
  emptyConversation,
  mergeEntries,
} from "@/adapters/observation";

type LoadOlder = (before: number, preserveScroll: () => void) => Promise<void>;

function paused(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    const finish = () => {
      clearTimeout(timer);
      signal.removeEventListener("abort", finish);
      resolve();
    };
    const timer = setTimeout(finish, ms);
    signal.addEventListener("abort", finish, { once: true });
  });
}

export function useConversation(target: Target | undefined) {
  const [view, setView] = useState(emptyConversation);
  const [attempt, setAttempt] = useState(0);
  const actions = useRef<{ loadOlder: LoadOlder; refresh: () => void } | null>(null);

  useEffect(() => {
    if (!target) return;
    const selected = target;
    const controller = new AbortController();
    const signal = controller.signal;
    const cancelled = () => signal.aborted;
    let cursor: number | undefined;
    let refreshing = false;
    let needsSnapshot = false;
    let readingHistory = false;

    async function refresh() {
      if (cursor === undefined) return;
      needsSnapshot = true;
      if (refreshing) return;
      refreshing = true;
      try {
        while (needsSnapshot && !cancelled()) {
          needsSnapshot = false;
          const snapshot = await fetchSnapshot(selected, signal);
          if (!cancelled())
            setView((value) => ({
              ...applySnapshot(value, snapshot),
              error: value.connection === "connected" ? undefined : value.error,
            }));
        }
      } catch (error) {
        if (!cancelled()) setView((value) => ({ ...value, error: errorMessage(error) }));
      } finally {
        refreshing = false;
      }
    }

    actions.current = {
      refresh: () => {
        void refresh();
      },
      loadOlder: async (before, preserveScroll) => {
        if (readingHistory || cancelled()) return;
        readingHistory = true;
        setView((value) => ({ ...value, loadingOlder: true, historyError: undefined }));
        try {
          const page = await fetchHistory(selected, before, signal);
          if (cancelled()) return;
          preserveScroll();
          setView((value) => ({
            ...value,
            entries: mergeEntries(
              value.entries,
              page.entries.filter(({ agent }) => agent === selected.agent),
            ),
            before: page.next_before,
            loadingOlder: false,
          }));
        } catch (error) {
          if (!cancelled())
            setView((value) => ({
              ...value,
              loadingOlder: false,
              historyError: errorMessage(error),
            }));
        } finally {
          readingHistory = false;
        }
      },
    };

    async function follow() {
      let backoff = 200;
      while (!cancelled()) {
        try {
          if (cursor === undefined) {
            const snapshot = await fetchSnapshot(selected, signal);
            const page = await fetchHistory(selected, snapshot.cursor + 1, signal);
            if (cancelled()) return;
            cursor = snapshot.cursor;
            setView({
              ...emptyConversation,
              snapshot,
              entries: page.entries.filter(({ agent }) => agent === selected.agent),
              before: page.next_before,
            });
          } else {
            void refresh();
          }
          const connected = () => {
            if (cancelled()) return;
            backoff = 200;
            setView((value) => ({
              ...value,
              connection: "connected",
              error: undefined,
              errorCode: undefined,
            }));
          };
          for await (const frame of observe(selected, cursor, signal, connected)) {
            if (cancelled()) return;
            if (frame.type === "committed") {
              if (frame.id <= cursor) continue;
              cursor = frame.id;
              if (frame.state_changed) void refresh();
            }
            if (frame.type === "lagged") void refresh();
            setView((value) => applyObservation(value, frame, selected.agent));
          }
          throw new Error("The observation stream closed.");
        } catch (error) {
          if (cancelled()) return;
          const definitive =
            error instanceof ApiError && [400, 401, 403, 404, 409, 422].includes(error.status);
          const connection = definitive ? "failed" : "reconnecting";
          setView((value) => ({
            ...value,
            connection,
            preview: null,
            error: errorMessage(error),
            errorCode: error instanceof ApiError ? error.code : undefined,
          }));
          if (definitive) return;
          await paused(backoff, signal);
          backoff = Math.min(backoff * 2, 5_000);
        }
      }
    }

    void follow();
    return () => {
      controller.abort();
      actions.current = null;
    };
  }, [target, attempt]);

  return {
    view,
    loadOlder: (preserveScroll: () => void) => {
      if (view.before !== null) void actions.current?.loadOlder(view.before, preserveScroll);
    },
    refresh: () => actions.current?.refresh(),
    reconnect: () => {
      setAttempt((count) => count + 1);
    },
  };
}
