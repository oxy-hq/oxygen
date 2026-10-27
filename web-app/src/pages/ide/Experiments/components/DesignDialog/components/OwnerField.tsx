import { Users } from "lucide-react";
import { useTeams } from "@/hooks/api/appAccess/useTeams";
import { useOrgMembers } from "@/hooks/api/organizations/useOrgMembers";
import useCurrentOrg from "@/stores/useCurrentOrg";
import type { Team } from "@/types/appAccess";
import type { OrgMember } from "@/types/organization";
import type { SectionProps } from "./fields";
import { type Match, PickerField, type PickerGroup } from "./PickerField";

/**
 * Who `owner` reaches, resolved the way the server's notifier does: a value with
 * an `@` is a member's email, anything else an org team's name, both matched
 * exactly. A value that reaches nobody (no match, or a team with no members)
 * still notifies the org's owners and admins, so it is a warning, not an error.
 */
export function ownerMatch(
  owner: string,
  members: OrgMember[] | undefined,
  teams: Team[] | undefined
): Match | null {
  const value = owner.trim();
  if (!value || (!members && !teams)) return null;
  if (value.includes("@")) {
    const m = members?.find((x) => x.email === value);
    if (m) return { tone: "success", text: `Member ${m.name || m.email}` };
  } else {
    const t = teams?.find((x) => x.name === value);
    if (t && t.member_count > 0) {
      return {
        tone: "success",
        text: `Team ${t.name} · ${t.member_count} member${t.member_count === 1 ? "" : "s"}`
      };
    }
    if (t) return { tone: "warning", text: `Team ${t.name} has no members` };
  }
  return {
    tone: "warning",
    text: "Matches no member or team: notices will go to the org's owners and admins"
  };
}

/**
 * The owner as a text field with the org's teams and members in a list under
 * it, filtered by what is typed, and a badge for who the value reaches. Free
 * text still saves, so a stored value that matches nobody stays editable.
 */
export default function OwnerField({ form, messageFor }: SectionProps) {
  const orgId = useCurrentOrg((s) => s.org?.id) ?? "";
  const { data: members } = useOrgMembers(orgId);
  const { data: teams } = useTeams(orgId);
  const value = form.watch("owner") ?? "";

  const groups: PickerGroup[] = [
    {
      title: "Teams",
      options: [...(teams ?? [])]
        .sort((a, b) => a.name.localeCompare(b.name))
        .map((t) => ({
          value: t.name,
          label: t.name,
          detail: `${t.member_count} member${t.member_count === 1 ? "" : "s"}`,
          search: t.name
        }))
    },
    {
      title: "Members",
      options: [...(members ?? [])]
        .sort((a, b) => a.email.localeCompare(b.email))
        .map((m) => ({
          value: m.email,
          label: m.email,
          detail: m.name || "",
          search: `${m.email} ${m.name ?? ""}`
        }))
    }
  ];

  return (
    <PickerField
      form={form}
      messageFor={messageFor}
      name='owner'
      label='Owned by'
      icon={Users}
      placeholder='Pick a team or a member'
      hint='The team or member notified about the run.'
      groups={groups}
      listLabel='Owner suggestions'
      match={ownerMatch(value, members, teams)}
    />
  );
}
