export const JWT_ISSUER = "__JWT_ISSUER__";
export async function verify(request: Request): Promise<boolean> {
  return request.headers.get("authorization")?.startsWith("Bearer ") ?? false;
}
