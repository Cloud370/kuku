import Ajv2020 from "ajv/dist/2020.js";

import contractSchema from "./generated/schema.json";
import type { TaskStreamEvent } from "./generated";

interface ContractSchema {
  $defs: Record<string, unknown>;
}

const schema = contractSchema as ContractSchema;
const taskStreamEventSchema = schema.$defs.TaskStreamEvent;

if (taskStreamEventSchema === undefined) {
  throw new Error("TaskStreamEvent is missing from the generated API schema");
}

const ajv = new Ajv2020({
  allErrors: true,
  logger: false,
  strict: false,
  validateFormats: false,
});
const validateTaskStreamEvent = ajv.compile<TaskStreamEvent>({
  ...taskStreamEventSchema,
  $defs: schema.$defs,
});

export class ContractDecodeError extends Error {
  readonly validationErrors: readonly string[];

  constructor(validationErrors: readonly string[]) {
    super("Task stream event does not match the API contract");
    this.name = "ContractDecodeError";
    this.validationErrors = validationErrors;
  }
}

export function decodeTaskStreamEvent(value: unknown): TaskStreamEvent {
  if (validateTaskStreamEvent(value)) {
    return value;
  }

  const validationErrors = (validateTaskStreamEvent.errors ?? []).map(
    (error) => `${error.instancePath || "/"} ${error.message ?? "is invalid"}`,
  );
  throw new ContractDecodeError(validationErrors);
}
