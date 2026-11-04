/** The user scope's root (ADR-0016 §1): habits that hold in every project, not a
 * directory. Matches `valkyrie_context::project::USER`. */
export const USER_SCOPE = "~user";

/** How a decision's scope is named: "your habits" for the user's, else the
 * project directory's name. */
export const scopeName = (path: string) =>
  path === USER_SCOPE ? "your habits" : (path.split("/").filter(Boolean).pop() ?? path);
