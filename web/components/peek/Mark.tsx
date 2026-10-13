/** Peek's app mark: a glass bubble resting on its information arc. */
export default function Mark({ size = 26 }: { size?: number }) {
  return (
    <svg className="mark" width={size} height={size} viewBox="0 0 64 64" aria-hidden="true">
      <rect width="64" height="64" rx="15" fill="#6c86c8" />
      <circle cx="32" cy="25" r="12.5" fill="#fff" />
      <path d="M11.5 38.5 Q32 55 52.5 38.5" fill="none" stroke="#fff" strokeWidth="5.5" strokeLinecap="round" />
    </svg>
  );
}
