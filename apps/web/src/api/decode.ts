import Ajv2020 from "ajv/dist/2020.js";

import contractSchema from "./generated/schema.json";
import type { ApiError, TaskStreamEvent } from "./generated";

interface ContractSchema {
  $defs: Record<string, unknown>;
}

const schema = contractSchema as ContractSchema;
const taskStreamEventSchema = schema.$defs.TaskStreamEvent;
const apiErrorSchema = schema.$defs.ApiError;

if (taskStreamEventSchema === undefined || apiErrorSchema === undefined) {
  throw new Error("Generated API schema is missing a required decoder definition");
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
const validateApiError = ajv.compile<ApiError>({
  ...apiErrorSchema,
  $defs: schema.$defs,
});

export class ContractDecodeError extends Error {
  readonly validationErrors: readonly string[];

  constructor(validationErrors: readonly string[]) {
    super("API contract validation failed");
    this.name = "ContractDecodeError";
    this.validationErrors = validationErrors;
  }
}

export function decodeTaskStreamEvent(value: unknown): TaskStreamEvent {
  if (validateTaskStreamEvent(value)) return value;

  throw contractDecodeError(validateTaskStreamEvent.errors);
}

export function decodeApiError(value: unknown): ApiError {
  if (validateApiError(value)) return value;

  throw contractDecodeError(validateApiError.errors);
}

function contractDecodeError(
  errors: readonly { instancePath: string; message?: string }[] | null | undefined,
): ContractDecodeError {
  const validationErrors = (errors ?? []).map(
    (error) => `${error.instancePath || "/"} ${error.message ?? "is invalid"}`,
  );
  return new ContractDecodeError(validationErrors);
}
