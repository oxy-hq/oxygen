import { Navigate, useLocation } from "react-router-dom";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { useAuth } from "@/contexts/AuthContext";
import { useKioskSessionGuard } from "@/hooks/auth/useKioskSessionGuard";
import { isOnOrgSubdomain, redirectToCentralLogin } from "@/libs/orgSubdomain";
import ROUTES from "@/libs/utils/routes";

interface ProtectedRouteProps {
  children: React.ReactNode;
}

const ProtectedRoute: React.FC<ProtectedRouteProps> = ({ children }) => {
  const { isAuthenticated, authConfig } = useAuth();
  const location = useLocation();
  // On an enrolled kiosk, a stored sign-in the session cookie no longer backs
  // is torn down before any signed-in page renders. A no-op everywhere else.
  const kioskSession = useKioskSessionGuard();

  if (!authConfig.auth_enabled) {
    return <>{children}</>;
  }

  if (!isAuthenticated()) {
    // On an org subdomain, auth is centralized on the app host — never render a
    // local /login (its OAuth redirect_uri would be the subdomain → provider
    // rejects it). Bounce to the app-host login. OrgSubdomainAuthGate normally
    // handles this on boot; this covers mid-session token loss.
    if (isOnOrgSubdomain() && redirectToCentralLogin()) {
      return null;
    }
    return <Navigate to={ROUTES.AUTH.LOGIN} state={{ from: location }} replace />;
  }

  if (kioskSession !== "ok") {
    return (
      <div
        className='flex h-full w-full items-center justify-center'
        data-testid='kiosk-session-checking'
      >
        <Spinner className='size-6' />
      </div>
    );
  }

  return <>{children}</>;
};

export default ProtectedRoute;
