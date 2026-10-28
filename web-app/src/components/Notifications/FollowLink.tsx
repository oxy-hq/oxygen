import type { ComponentProps } from "react";
import { Link } from "react-router-dom";
import { isAppHref } from "./utils";

type FollowLinkProps = Omit<ComponentProps<"a">, "href" | "target"> & { href: string };

/**
 * Where a notification points: a custom app through a plain anchor (its own
 * document, in this tab), a platform path through the router.
 */
export function FollowLink({ href, children, ...props }: FollowLinkProps) {
  if (isAppHref(href)) {
    return (
      <a href={href} {...props}>
        {children}
      </a>
    );
  }
  return (
    <Link to={href} {...props}>
      {children}
    </Link>
  );
}
