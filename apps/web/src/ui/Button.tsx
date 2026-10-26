import { cva, type VariantProps } from "class-variance-authority";
import { Slot } from "radix-ui";
import { forwardRef, type ButtonHTMLAttributes } from "react";
import { cn } from "./cn";

export const buttonVariants = cva(
  "inline-flex shrink-0 items-center justify-center gap-2 whitespace-nowrap rounded-full font-medium select-none " +
    "transition-[background-color,color,transform,opacity] duration-150 ease-soft active:scale-[0.97] " +
    "disabled:pointer-events-none disabled:opacity-45 [&_svg]:size-[1.15em] [&_svg]:shrink-0",
  {
    variants: {
      variant: {
        primary: "bg-accent text-on-accent hover:bg-accent-hover",
        secondary: "bg-surface text-fg hover:bg-line/70",
        ghost: "text-fg hover:bg-surface",
        quiet: "text-muted hover:bg-surface hover:text-fg",
        danger: "bg-danger text-on-danger hover:opacity-90",
        /** Over photos: a dark frosted pill that reads on any picture. */
        overlay: "bg-black/35 text-white backdrop-blur-md hover:bg-black/50",
      },
      size: {
        sm: "h-8 px-3 text-[0.8125rem]",
        md: "h-10 px-4 text-[0.9375rem]",
        lg: "h-12 px-6 text-base",
        icon: "size-10 text-[1.125rem]",
        "icon-sm": "size-8 text-base",
      },
    },
    defaultVariants: { variant: "secondary", size: "md" },
  },
);

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement>, VariantProps<typeof buttonVariants> {
  /** Render the child (a link, say) with the button's look instead of a `<button>`. */
  asChild?: boolean;
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  { className, variant, size, asChild, type, ...props },
  ref,
) {
  const Comp = asChild ? Slot.Root : "button";
  return (
    <Comp
      ref={ref}
      type={asChild ? undefined : (type ?? "button")}
      className={cn(buttonVariants({ variant, size }), className)}
      {...props}
    />
  );
});
