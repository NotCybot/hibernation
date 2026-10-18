import { invalidate } from '$app/navigation';
import { createWebsocket } from '@profidev/pleiades/backend';

export enum UpdateType {
  Settings = 'Settings',
  User = 'User',
  UserPermissions = 'UserPermissions',
  Group = 'Group',
  Token = 'Token',
  Cache = 'Cache'
}

export type UpdateMessage =
  | {
      type:
        | UpdateType.User
        | UpdateType.Group
        | UpdateType.Token
        | UpdateType.Cache;
      uuid: string;
    }
  | {
      type: UpdateType.Settings | UpdateType.UserPermissions;
    };

const socket = createWebsocket<UpdateMessage>();

export const connectWebsocket = (user: string) =>
  socket.connect((msg) => handleMessage(msg, user));
export const disconnectWebsocket = () => socket.disconnect();

const handleMessage = (msg: UpdateMessage, user: string) => {
  switch (msg.type) {
    case UpdateType.Settings: {
      invalidate((url) => url.pathname.startsWith('/api/settings')).catch(
        () => {}
      );
      break;
    }
    case UpdateType.User: {
      invalidate('/api/user/management').catch(() => {});
      invalidate(`/api/user/management/${msg.uuid}`).catch(() => {});
      invalidate('/api/group/users').catch(() => {});
      // Same as current user
      if (msg.uuid === user) {
        invalidate('/api/user/info').catch(() => {});
      }
      break;
    }
    case UpdateType.UserPermissions: {
      invalidate('/api/user/info').catch(() => {});
      break;
    }
    case UpdateType.Group: {
      invalidate('/api/group').catch(() => {});
      invalidate(`/api/group/${msg.uuid}`).catch(() => {});
      invalidate('/api/user/management/groups').catch(() => {});
      break;
    }
    case UpdateType.Token: {
      invalidate('/api/token').catch(() => {});
      invalidate(`/api/token/${msg.uuid}`).catch(() => {});
      break;
    }
    case UpdateType.Cache: {
      invalidate('/api/cache/management').catch(() => {});
      invalidate(`/api/cache/management/${msg.uuid}`).catch(() => {});
      invalidate('/api/group/caches').catch(() => {});
      break;
    }
    default: {
      break;
    }
  }
};
