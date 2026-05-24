export interface Turn {
  turnNumber: number;
  userText: string;
  agent: AgentMessage;
  status: "streaming" | "complete" | "error" | "cancelled";
}

export interface AgentMessage {
  thinking?: string;
  text?: string;
  model?: { provider: string; model: string };
  tools: ToolRender[];
  permissions: PermissionRender[];
  error?: { code: string; message: string };
}

export interface ToolRender {
  id: string;
  name: string;
  summary: string;
  kind: "simple" | "agent" | "command";
  childSessionId?: string;
  status: "running" | "completed" | "error" | "blocked";
  modelContent?: string;
  result?: unknown;
  subEvents: SubEvent[];
}

export type SubEvent =
  | { type: "text"; content: string }
  | { type: "thinking"; content: string }
  | { type: "stdout"; text: string }
  | { type: "stderr"; text: string }
  | { type: "tool_start"; id: string; tool: string; summary: string }
  | { type: "tool_end"; id: string; status: string; summary: string };

export interface PermissionRender {
  id: string;
  tool: string;
  risk: string;
  summary: string;
}
