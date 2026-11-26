/**
 * Device desired-state config validation (docs/device-config-schema.md).
 *
 * Uses the SAME JSON Schema artifact as the Rust CLI
 * (`schemas/device-config.schema.json`, Draft 2020-12) via AJV — there is
 * deliberately no second handwritten structural validator. Semantic /
 * device-capability checks live in `core/capabilities.ts` and the safety
 * layer; this module is layer 1 only.
 */
import Ajv2020 from "ajv/dist/2020.js";
import type { ErrorObject } from "ajv";
// The single structural-truth artifact — imported, never copied.
// `with { type: "json" }` keeps this module loadable under Node's native
// TS runner (smoke tests) as well as Vite.
import schema from "../../schemas/device-config.schema.json" with { type: "json" };

const ajv = new Ajv2020({ allErrors: true, strict: false });
const validate = ajv.compile(schema as object);

export interface DeviceConfigIssue {
  /** JSON path to the offending value ("" = root). */
  path: string;
  message: string;
}

export interface DeviceConfigResult {
  valid: boolean;
  issues: DeviceConfigIssue[];
}

/** Structural validation only — says nothing about apply-ability. */
export function validateDeviceConfig(doc: unknown): DeviceConfigResult {
  const ok = validate(doc);
  if (ok) return { valid: true, issues: [] };
  const issues = (validate.errors ?? []).map((e: ErrorObject) => ({
    path: e.instancePath || "/",
    message: e.message ?? "invalid",
  }));
  return { valid: false, issues };
}

/** The artifact itself — for "copy AI instructions" / docs display. */
export function deviceConfigSchema(): object {
  return schema as object;
}

export const DEVICE_CONFIG_SCHEMA_ID =
  (schema as { $id?: string }).$id ?? "device-config.schema.json";
