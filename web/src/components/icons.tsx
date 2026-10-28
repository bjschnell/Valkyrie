// Small inline icons, stroked in the current color.

const base = {
  width: 20,
  height: 20,
  viewBox: "0 0 24 24",
  fill: "none",
  stroke: "currentColor",
  strokeWidth: 2,
  strokeLinecap: "round" as const,
  strokeLinejoin: "round" as const,
  "aria-hidden": true,
};

export const Back = () => (
  <svg {...base}>
    <path d="M15 18l-6-6 6-6" />
  </svg>
);
export const Send = () => (
  <svg {...base}>
    <path d="M5 12h14M13 6l6 6-6 6" />
  </svg>
);
export const Check = () => (
  <svg {...base}>
    <path d="M5 12l5 5L20 7" />
  </svg>
);
export const Wrap = () => (
  <svg {...base}>
    <path d="M4 6h16M4 12h13a3 3 0 010 6h-4m0 0l2-2m-2 2l2 2M4 18h5" />
  </svg>
);
export const Grid = () => (
  <svg {...base}>
    <rect x="3" y="4" width="18" height="16" rx="2" />
    <path d="M7 9h4M7 13h8M7 17h3" />
  </svg>
);
export const Chevron = () => (
  <svg {...base} width={16} height={16}>
    <path d="M9 6l6 6-6 6" />
  </svg>
);
export const Dots = () => (
  <svg {...base}>
    <circle cx="5" cy="12" r="1" />
    <circle cx="12" cy="12" r="1" />
    <circle cx="19" cy="12" r="1" />
  </svg>
);
export const Bell = () => (
  <svg {...base}>
    <path d="M6 9a6 6 0 0112 0c0 5 2 6.5 2 6.5H4S6 14 6 9zM10 19.5a2 2 0 004 0" />
  </svg>
);
export const Mic = () => (
  <svg {...base}>
    <rect x="9" y="3" width="6" height="11" rx="3" />
    <path d="M5 11a7 7 0 0014 0M12 18v3" />
  </svg>
);
export const Clip = () => (
  <svg {...base}>
    <path d="M21 11.5l-8.6 8.6a5 5 0 01-7.1-7.1l8.6-8.6a3.3 3.3 0 014.7 4.7l-8.6 8.6a1.7 1.7 0 01-2.4-2.4l7.9-7.9" />
  </svg>
);
export const List = () => (
  <svg {...base}>
    <path d="M9 6h11M9 12h11M9 18h11M4.5 6h.01M4.5 12h.01M4.5 18h.01" />
  </svg>
);
export const Plus = () => (
  <svg {...base}>
    <path d="M12 5v14M5 12h14" />
  </svg>
);
export const Diff = () => (
  <svg {...base}>
    <path d="M12 4v8M8 8h8M8 18h8" />
  </svg>
);
export const Gear = () => (
  <svg {...base}>
    <circle cx="12" cy="12" r="3" />
    <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
  </svg>
);
