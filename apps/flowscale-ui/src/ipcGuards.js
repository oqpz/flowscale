const isRecord = value => typeof value === 'object' && value !== null && !Array.isArray(value);

export function isHostEventShape(value) {
  if (!isRecord(value) || typeof value.type !== 'string' || !isRecord(value.payload)) return false;
  const payload = value.payload;
  switch (value.type) {
    case 'state_sync':
      return isRecord(payload.settings) && Array.isArray(payload.profiles) &&
        isRecord(payload.telemetry) && isRecord(payload.hardware) && isRecord(payload.engine_config);
    case 'telemetry_tick':
      return typeof payload.status === 'string';
    case 'countdown_tick':
      return Number.isFinite(payload.seconds);
    case 'picked_executable':
      return typeof payload.path === 'string';
    case 'profile_upserted':
      return typeof payload.id === 'string' && typeof payload.revision === 'string' &&
        payload.revision.length <= 64 && typeof payload.ok === 'boolean' &&
        (!payload.ok || (isRecord(payload.profile) && payload.profile.id === payload.id));
    case 'configs_result':
      return payload.entries === null || (Array.isArray(payload.entries) &&
        payload.entries.every(entry => isRecord(entry) && Number.isSafeInteger(entry.id) &&
          typeof entry.name === 'string'));
    case 'toast':
      return ['success', 'error', 'warning', 'info'].includes(payload.kind) &&
        typeof payload.message === 'string';
    default:
      return false;
  }
}
