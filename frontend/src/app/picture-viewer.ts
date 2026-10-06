// A picture, full screen over the thread. Opened through the scaffold's `Dialogs`,
// so back closes it and the thread stays where it was.

import { Component, inject } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MAT_DIALOG_DATA, MatDialogRef } from '@angular/material/dialog';
import { MatIconModule } from '@angular/material/icon';

export interface PictureData {
  readonly src: string;
  readonly alt: string;
}

@Component({
  selector: 'app-picture-viewer',
  templateUrl: './picture-viewer.html',
  styleUrl: './picture-viewer.scss',
  imports: [MatButtonModule, MatIconModule],
})
export class PictureViewer {
  protected readonly data = inject<PictureData>(MAT_DIALOG_DATA);
  protected readonly ref = inject(MatDialogRef<PictureViewer>);
}
