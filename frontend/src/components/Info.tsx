import React from "react";
import { Info as InfoIcon } from "lucide-react";
import { Dialog, DialogContent, DialogTitle, DialogTrigger } from "./ui/dialog";
import { VisuallyHidden } from "./ui/visually-hidden";
import { About } from "./About";

interface InfoProps {
    isCollapsed: boolean;
}

const Info = React.forwardRef<HTMLButtonElement, InfoProps>(({ isCollapsed }, ref) => {
  return (
    <Dialog aria-describedby={undefined}>
      <DialogTrigger asChild>
        <button
          ref={ref}
          className={`flex items-center justify-center cursor-pointer border-none transition-colors ${
            isCollapsed
              ? "h-10 w-10 rounded-xl bg-transparent text-muted-foreground hover:text-foreground"
              : "w-full gap-2 rounded-xl border border-border/10 bg-secondary/5 px-3 py-2 text-sm font-medium text-foreground hover:bg-secondary/10"
          }`}
          title="About Meetily"
        >
          <InfoIcon className={isCollapsed ? "w-5 h-5" : "w-4 h-4"} />
          {!isCollapsed && (
            <span className="ml-2 text-sm">About</span>
          )}
        </button>
      </DialogTrigger>
      <DialogContent>
        <VisuallyHidden>
          <DialogTitle>About Meetily</DialogTitle>
        </VisuallyHidden>
        <About />
      </DialogContent>
    </Dialog>
  );
});

Info.displayName = "About";

export default Info; 