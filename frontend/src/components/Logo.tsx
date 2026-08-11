import React from "react";
import Image from "next/image";
import { Dialog, DialogContent, DialogTitle, DialogTrigger } from "./ui/dialog";
import { VisuallyHidden } from "./ui/visually-hidden";
import { About } from "./About";

interface LogoProps {
    isCollapsed: boolean;
}

const Logo = React.forwardRef<HTMLButtonElement, LogoProps>(({ isCollapsed }, ref) => {
  return (
    <Dialog aria-describedby={undefined}>
      {isCollapsed ? (
        <DialogTrigger asChild>
          <button
            ref={ref}
            className="flex h-[34px] w-[34px] items-center justify-center rounded-lg border border-border/10 bg-gradient-to-br from-white/10 to-white/0 cursor-pointer hover:opacity-80 transition-opacity"
          >
            <Image src="/logo-collapsed.png" alt="Logo" width={22} height={18} className="object-contain" />
          </button>
        </DialogTrigger>
      ) : (
        <DialogTrigger asChild>
          <span className="glass-pill inline-flex items-center justify-center w-full px-4 py-1.5 mb-2 text-sm font-semibold text-foreground cursor-pointer hover:bg-secondary/10 transition-colors">
            <span>Meetily</span>
          </span>
        </DialogTrigger>
      )}
      <DialogContent>
        <VisuallyHidden>
          <DialogTitle>About Meetily</DialogTitle>
        </VisuallyHidden>
        <About />
      </DialogContent>
    </Dialog>
  );
});

Logo.displayName = "Logo";

export default Logo;