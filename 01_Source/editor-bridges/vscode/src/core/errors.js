'use strict';
// Shared error model (DD §2.2 / §12.2). Errors carry a stable code and a user action,
// never source text, prompts or credentials.
class PimeError extends Error {
  constructor(code, message, {severity = 'error', retryable = false, userAction = ''} = {}) {
    super(message || code);
    this.name = 'PimeError';
    this.code = code;
    this.severity = severity;
    this.retryable = retryable;
    this.userAction = userAction;
  }
}
const CODES = Object.freeze([
  'PROFILE_NOT_REGISTERED', 'IPC_TIMEOUT', 'SECURE_INPUT', 'CONSENT_REQUIRED', 'MODEL_UNSUPPORTED',
  'AI_RATE_LIMIT', 'AI_BAD_REQUEST', 'AI_AUTH_FAILED', 'AI_QUOTA', 'AI_SERVER_ERROR', 'AI_TIMEOUT', 'AI_NETWORK',
  'AI_BUDGET_EXCEEDED', 'AI_DISABLED', 'AI_CANCELLED', 'STALE_RESULT', 'STALE_DOCUMENT', 'STALE_SELECTION',
  'PATCH_INVALID', 'EDIT_UNKNOWN', 'EDIT_REJECTED', 'BYTE_ENCODING_UNRESOLVED', 'VAULT_ERROR',
  'OUTPUT_INVALID', 'TARGET_RANGE_INVALID', 'PAYLOAD_TOO_LARGE', 'CONTEXT_UNAVAILABLE',
  'FULL_CONTEXT_REQUIRED', 'CROSS_FILE_DENIED', 'VALIDATION_FAILED', 'GRANT_DENIED', 'TOKEN_INVALID',
  'USER_CONFIRMATION_REQUIRED', 'SENSITIVE_BLOCKED', 'DOCUMENT_CLOSED', 'RULEPACK_INVALID', 'SETTINGS_INVALID'
]);
const fail = (code, message, meta) => new PimeError(code, message, meta);
const codeOf = e => (e && typeof e.code === 'string' && CODES.includes(e.code)) ? e.code : 'INTERNAL';
module.exports = {PimeError, CODES, fail, codeOf};
