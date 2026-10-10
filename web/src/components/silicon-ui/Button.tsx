import { splitProps, type JSX } from "solid-js";
import styles from "./button.module.css";

/** Solid adapter for Silicon UI's public button styles. */
export default function Button(props: JSX.ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: "primary" | "secondary" | "ghost" | "danger";
  loading?: boolean;
}) {
  const [local, rest] = splitProps(props, ["variant", "loading", "children", "class", "disabled"]);
  return <button {...rest} class={`${styles.button} ${styles[local.variant ?? "primary"]} ${styles.md} ${local.class ?? ""}`}
    disabled={local.disabled || local.loading} aria-busy={local.loading || undefined}>
    {local.children}
  </button>;
}
