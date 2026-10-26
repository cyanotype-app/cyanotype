import { DropdownMenu } from "radix-ui";
import { forwardRef, type ComponentPropsWithoutRef, type ReactNode } from "react";
import { cn } from "./cn";

/**
 * A dropdown menu (Radix). Always non-modal: the modal kind locks scrolling by injecting a style element, which the
 * gateway's CSP refuses. Outside clicks and Escape still close it, and focus still moves through its items.
 */
export function Menu({ trigger, children, align = "end", label }: { trigger: ReactNode; children: ReactNode; align?: "start" | "end"; label?: string }) {
  return (
    <DropdownMenu.Root modal={false}>
      <DropdownMenu.Trigger asChild>{trigger}</DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          align={align}
          sideOffset={6}
          collisionPadding={8}
          aria-label={label}
          className={cn(
            "z-50 min-w-48 max-w-[min(20rem,var(--radix-dropdown-menu-content-available-width))] overflow-hidden rounded-card border border-line bg-card p-1.5 text-fg shadow-float",
            "origin-(--radix-dropdown-menu-content-transform-origin) data-[state=open]:animate-[menu-in_140ms_var(--ease-soft)]",
          )}
        >
          {children}
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

const itemClass =
  "flex min-h-10 cursor-default items-center gap-3 rounded-[0.55rem] px-2.5 text-[0.9375rem] outline-none select-none " +
  "data-[highlighted]:bg-surface data-[disabled]:opacity-45 [&_svg]:size-[1.1rem] [&_svg]:shrink-0 [&_svg]:text-muted";

export const MenuItem = forwardRef<HTMLDivElement, ComponentPropsWithoutRef<typeof DropdownMenu.Item> & { danger?: boolean }>(
  function MenuItem({ className, danger, ...props }, ref) {
    return <DropdownMenu.Item ref={ref} className={cn(itemClass, danger && "text-danger [&_svg]:text-danger", className)} {...props} />;
  },
);

export function MenuLabel({ children }: { children: ReactNode }) {
  return <DropdownMenu.Label className="truncate px-2.5 pt-1.5 pb-1 text-xs text-muted">{children}</DropdownMenu.Label>;
}

export function MenuSeparator() {
  return <DropdownMenu.Separator className="my-1.5 h-px bg-line" />;
}
