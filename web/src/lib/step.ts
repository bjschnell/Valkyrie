import { STEP_LABEL, type Flag, type StepAction } from "../proto";

/** A button on a flagged card: what it takes, its label, and whether it's the one offered. */
export interface StepChoice {
  action: StepAction;
  label: string;
  primary: boolean;
}

/**
 * The buttons a flagged session's card shows (ADR-0019 §2): the step offered, the
 * other that would fit, and leave. None until the supervisor has picked its step.
 */
export function stepChoices(flag: Flag | undefined): StepChoice[] {
  const step = flag?.step;
  if (!step) return [];
  const actions: StepAction[] = [step.action];
  if (step.alt && step.alt !== step.action) actions.push(step.alt);
  if (!actions.includes("leave")) actions.push("leave");
  return actions.map((action, i) => ({
    action,
    label: STEP_LABEL[action],
    primary: i === 0 && action !== "leave",
  }));
}
