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
export const BellOff = () => (
  <svg {...base}>
    <path d="M8.5 4.2A6 6 0 0118 9c0 2.3.4 3.8.9 4.8M17 15.5H4S6 14 6 9c0-.6.1-1.2.3-1.8M10 19.5a2 2 0 004 0M3 3l18 18" />
  </svg>
);
export const Mic = () => (
  <svg {...base}>
    <rect x="9" y="3" width="6" height="11" rx="3" />
    <path d="M5 11a7 7 0 0014 0M12 18v3" />
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
