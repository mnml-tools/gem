// Synthetic TypeScript module used by the Gem test servers.
export interface User {
  id: number;
  name: string;
  roles: readonly string[];
}

export function greet(user: User): string {
  const admin = user.roles.includes('admin') ? ' (admin)' : '';
  return `Hello, ${user.name}${admin}`;
}

export async function loadUsers(url: string): Promise<User[]> {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`HTTP ${response.status}`);
  return (await response.json()) as User[];
}
