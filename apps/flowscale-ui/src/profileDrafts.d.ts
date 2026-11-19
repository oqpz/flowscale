import type {ScalingProfile} from './types';

export function mergePendingProfiles(
  snapshot: ScalingProfile[],
  pending: Map<string, {timerId: number; profile: ScalingProfile}>,
  inFlight?: Map<string, {revision: string; profile: ScalingProfile}>,
): ScalingProfile[];

export function takePendingProfiles(
  pending: Map<string, {timerId: number; profile: ScalingProfile}>,
): Array<{timerId: number; profile: ScalingProfile}>;

export function acknowledgeProfileDraft(
  inFlight: Map<string, {revision: string; profile: ScalingProfile}>,
  id: string,
  revision: string,
): boolean;
