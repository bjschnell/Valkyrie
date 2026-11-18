/**
 * Whether the rain falls: not while the tab is hidden, where nobody sees it, and
 * not while a card needs you, so it doesn't move behind the work.
 */
export const rainFalls = (hidden: boolean, waiting: number): boolean => !hidden && waiting === 0;
