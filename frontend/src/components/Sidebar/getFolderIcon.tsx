import React, { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import {
  Folder,
  Gamepad2,
  Briefcase,
  Scale,
  BookOpen,
  Heart,
  Share2,
  Code,
  DollarSign,
  GraduationCap,
  Users,
  Film,
  Music,
  ShoppingBag,
  Target,
  Wrench,
  Globe,
  ShieldAlert,
  FolderArchive,
  FolderKanban,
  FileText,
  Building,
  Brain,
  Lightbulb,
  Cpu,
} from 'lucide-react';

const ICON_MAP: Record<string, React.ComponentType<{ className?: string }>> = {
  scale: Scale,
  gamepad: Gamepad2,
  book: BookOpen,
  heart: Heart,
  share: Share2,
  code: Code,
  dollar: DollarSign,
  'graduation-cap': GraduationCap,
  users: Users,
  briefcase: Briefcase,
  brain: Brain,
  lightbulb: Lightbulb,
  film: Film,
  music: Music,
  'shopping-bag': ShoppingBag,
  target: Target,
  wrench: Wrench,
  globe: Globe,
  shield: ShieldAlert,
  archive: FolderArchive,
  kanban: FolderKanban,
  'file-text': FileText,
  building: Building,
  cpu: Cpu,
  folder: Folder,
};

const iconCache = new Map<string, string>();

function renderLucideIcon(iconKey: string | undefined, defaultIcon: React.ComponentType<{ className?: string }>, className: string) {
  const IconComponent = (iconKey && ICON_MAP[iconKey]) || defaultIcon;
  return <IconComponent className={className} />;
}

export function DynamicIcon({ 
  id, 
  name, 
  existingIcon, 
  isFolder, 
  className 
}: { 
  id?: string;
  name: string; 
  existingIcon?: string;
  isFolder: boolean; 
  className: string;
}) {
  const cacheKey = `${isFolder ? 'f' : 'm'}:${name}`;
  const [iconName, setIconName] = useState<string | undefined>(existingIcon || iconCache.get(cacheKey));

  useEffect(() => {
    // If we already have an icon (from DB or cache), use it and stop.
    if (existingIcon) {
      setIconName(existingIcon);
      return;
    }
    
    if (iconCache.has(cacheKey)) {
      setIconName(iconCache.get(cacheKey));
      return;
    }

    let isMounted = true;
    invoke<string>('api_recommend_icon', { name, isFolder })
      .then((res) => {
        if (res && res !== 'default') {
          iconCache.set(cacheKey, res);
          if (isMounted) setIconName(res);
          
          // Save to database if we have an ID
          if (id) {
            if (isFolder) {
              invoke('api_update_folder_icon', { id, icon: res }).catch(console.error);
            } else {
              invoke('api_update_meeting_icon', { id, icon: res }).catch(console.error);
            }
          }
        }
      })
      .catch(() => {});

    return () => {
      isMounted = false;
    };
  }, [id, name, existingIcon, isFolder, cacheKey]);

  const defaultComponent = isFolder ? Folder : FileText;
  return renderLucideIcon(iconName, defaultComponent, className);
}

export function getFolderIcon(folderName: string, className = "w-4 h-4 mr-2 text-muted-foreground", id?: string, existingIcon?: string): React.ReactNode {
  return <DynamicIcon id={id} name={folderName} existingIcon={existingIcon} isFolder={true} className={className} />;
}

export function getMeetingIcon(meetingTitle: string, className = "w-3.5 h-3.5 text-muted-foreground", id?: string, existingIcon?: string): React.ReactNode {
  return <DynamicIcon id={id} name={meetingTitle} existingIcon={existingIcon} isFolder={false} className={className} />;
}
