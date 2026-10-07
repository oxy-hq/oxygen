import type React from "react";
import useTheme from "@/stores/useTheme";

/**
 * The frame every signed-out screen shares: the lockup in the corner and one
 * card in the middle. Sign-in, a sent link, a one-time link's three outcomes —
 * each is that card with different words in it, so a person (or a script) that
 * has seen one has seen the shape of all of them.
 */
export const AuthLayout = ({ children }: { children: React.ReactNode }) => {
  const { theme } = useTheme();

  return (
    <div className='grid h-full min-h-screen w-full overflow-auto bg-background'>
      <div className='flex flex-col gap-4 p-6 md:p-10'>
        <div className='flex justify-center gap-2 md:justify-start'>
          <a href='/' className='flex items-center gap-2 font-medium'>
            <img src={theme === "dark" ? "/oxygen-dark.svg" : "/oxygen-light.svg"} alt='Oxygen' />
            <span className='truncate font-medium text-sm'>Oxygen</span>
          </a>
        </div>
        <div className='flex flex-1 items-center justify-center'>
          <div className='w-full max-w-sm'>{children}</div>
        </div>
      </div>
    </div>
  );
};

interface AuthCardProps {
  title: string;
  description?: React.ReactNode;
  testId?: string;
  children?: React.ReactNode;
}

/** The card itself: a title, at most one line under it, then the controls. */
export const AuthCard = ({ title, description, testId, children }: AuthCardProps) => (
  <div className='flex flex-col gap-5 rounded-xl border bg-card p-6' data-testid={testId}>
    <div className='flex flex-col gap-1'>
      <h1 className='font-semibold text-xl'>{title}</h1>
      {description && <p className='text-muted-foreground text-sm'>{description}</p>}
    </div>
    {children}
  </div>
);
