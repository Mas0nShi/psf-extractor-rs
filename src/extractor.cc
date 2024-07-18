#include "psf_extractor/include/extractor.h"
#include "psf_extractor/src/lib.rs.h"

#include <windows.h>
#include <sys/stat.h>
#include <stdlib.h>
#include <io.h>
#include <fcntl.h>
#include <fdi.h>

#pragma comment(lib, "Cabinet.lib")

#define MAX_PATH_W 32767

// global variables
char* TargetDirectoryName;



// Cabinet API functions
FNALLOC(FDIAlloc) {
	return HeapAlloc(GetProcessHeap(), 0, cb);
}

FNFREE(FDIFree) {
	HeapFree(GetProcessHeap(), 0, pv);
}

static inline WCHAR* strdupAtoW(UINT cp, const char* str) {
	WCHAR* res = NULL;
	if (str) {
		DWORD len = MultiByteToWideChar(cp, 0, str, -1, NULL, 0) + 2;
		if ((res = (WCHAR*)FDIAlloc(sizeof(WCHAR) * len))) {
			MultiByteToWideChar(cp, 0, str, -1, res, len);
		}
	}
	return res;
}

void CreateDirectoryRecursive(const WCHAR* name) {
	WCHAR* path, * p;
	path = (WCHAR*)FDIAlloc((lstrlenW(name) + 1) * sizeof(WCHAR));
	lstrcpyW(path, name);
	p = wcschr(path, '\\');
	while (p != NULL) {
		*p = 0;
		CreateDirectoryW(path, NULL);
		*p = '\\';
		p = wcschr(p + 1, '\\');
	}
	FDIFree(path);
}

FNOPEN(FDIOpen) {
	HANDLE hf = NULL;
	DWORD dwDesiredAccess = 0;
	DWORD dwCreationDisposition = 0;
	UNREFERENCED_PARAMETER(pmode);
	if (oflag & _O_RDWR) {
		dwDesiredAccess = GENERIC_READ | GENERIC_WRITE;
	}
	else if (oflag & _O_WRONLY) {
		dwDesiredAccess = GENERIC_WRITE;
	}
	else {
		dwDesiredAccess = GENERIC_READ;
	}
	if (oflag & _O_CREAT) {
		dwCreationDisposition = CREATE_ALWAYS;
	}
	else {
		dwCreationDisposition = OPEN_EXISTING;
	}
	WCHAR* pszFileW = strdupAtoW(CP_ACP, pszFile);
	hf = CreateFileW(pszFileW, dwDesiredAccess, FILE_SHARE_READ, NULL, dwCreationDisposition, FILE_ATTRIBUTE_NORMAL, NULL);
	FDIFree(pszFileW);
	return (INT_PTR)hf;
}

FNREAD(FDIRead) {
	DWORD num_read;
	if (!ReadFile((HANDLE)hf, pv, cb, &num_read, NULL)) {
		return -1;
	}
	return num_read;
}

FNWRITE(FDIWrite) {
	DWORD written;
	if (!WriteFile((HANDLE)hf, pv, cb, &written, NULL)) {
		return -1;
	}
	return written;
}

FNCLOSE(FDIClose) {
	if (!CloseHandle((HANDLE)hf)) {
		return -1;
	}
	return 0;
}

FNSEEK(FDISeek) {
	DWORD res;
	res = SetFilePointer((HANDLE)hf, dist, NULL, seektype);
	if (res == INVALID_SET_FILE_POINTER && GetLastError()) {
		return -1;
	}
	return res;
}

FNFDINOTIFY(FDINotify) {
	WCHAR* nameW = NULL, * file = NULL, * TargetDirectoryNameW = NULL, * TargetFilePath = NULL;
	FILETIME FileTime = { 0 };
	HANDLE hf = NULL;
	switch (fdint) {
	case fdintCABINET_INFO:
		return 0;
	case fdintCOPY_FILE:
		nameW = strdupAtoW((pfdin->attribs & _A_NAME_IS_UTF) ? CP_UTF8 : CP_ACP, pfdin->psz1);
		file = nameW;
		while (*file == '\\') {
			file++;
		}
		TargetDirectoryNameW = strdupAtoW(CP_ACP, TargetDirectoryName);
		TargetFilePath = (WCHAR*)FDIAlloc((lstrlenW(TargetDirectoryNameW) + lstrlenW(file) + 1) * sizeof(WCHAR));
		wcscpy_s(TargetFilePath, (lstrlenW(TargetDirectoryNameW) + lstrlenW(file) + 1), TargetDirectoryNameW);
		wcscat_s(TargetFilePath, (lstrlenW(TargetDirectoryNameW) + lstrlenW(file) + 1), file);
		CreateDirectoryRecursive(TargetFilePath);
		hf = CreateFileW(TargetFilePath, GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE, NULL, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
		FDIFree(nameW);
		FDIFree(TargetDirectoryNameW);
		FDIFree(TargetFilePath);
		return (INT_PTR)hf;
	case fdintCLOSE_FILE_INFO:
		DosDateTimeToFileTime(pfdin->date, pfdin->time, &FileTime);
		SetFileTime((HANDLE)(pfdin->hf), NULL, NULL, &FileTime);
		FDIClose(pfdin->hf);
		// CurrentFiles++;
		// UpdateProgressBar((float)CurrentFiles / (float)TotalFiles);
		return TRUE;
	case fdintPARTIAL_FILE:
		return -1;
	case fdintENUMERATE:
		return 0;
	case fdintNEXT_CABINET:
		return -1;
	default:
		return 0;
	}
}


bool extract(rust::Str file_name, rust::Str file_dir, rust::Str out_path) {
	int fileNameLength = file_name.size();
	int fileDirLength = file_dir.size();
	int outPathLength = out_path.size();

    char *CABFilePart = (char*)FDIAlloc(sizeof(char) * (fileNameLength + 1));
    strncpy(CABFilePart, file_name.data(), fileNameLength);
    CABFilePart[fileNameLength] = '\0';

    char *CABPathPart = (char*)FDIAlloc(sizeof(char) * (fileDirLength + 2));
    strncpy(CABPathPart, file_dir.data(), fileDirLength);
	strncpy(CABPathPart + fileDirLength, "\\\0", 2);

    TargetDirectoryName = (char*)FDIAlloc(sizeof(char) * MAX_PATH_W);
	if (TargetDirectoryName == nullptr) return false;
    strncpy(TargetDirectoryName, out_path.data(), outPathLength);
	strncpy(TargetDirectoryName + outPathLength, "\\\0", 2);

	// Create FDI context
	ERF* FDIErf = (ERF*)FDIAlloc(sizeof(ERF));
	HFDI FDIContext = FDICreate(FDIAlloc, FDIFree, FDIOpen, FDIRead, FDIWrite, FDIClose, FDISeek, cpuUNKNOWN, FDIErf);
	if (FDIContext == NULL) {
		return false;
	}

	// Get file number of CAB file
	char* CABFileFullName = (char*)FDIAlloc(sizeof(char) * MAX_PATH_W);
	strcpy_s(CABFileFullName, MAX_PATH_W, CABPathPart);
	strcat_s(CABFileFullName, MAX_PATH_W, CABFilePart);
	WCHAR* CABFileFullNameW = strdupAtoW(CP_ACP, CABFileFullName);
	FDIFree(CABFileFullName);

	HANDLE hf = CreateFileW(CABFileFullNameW, GENERIC_READ, FILE_SHARE_READ, NULL, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, NULL);
	if (hf == INVALID_HANDLE_VALUE) {
		return false;
	}

    FDIFree(CABFileFullNameW);
	FDICABINETINFO* CabinetInfo = (FDICABINETINFO*)FDIAlloc(sizeof(FDICABINETINFO));
	if (!FDIIsCabinet(FDIContext, (INT_PTR)hf, CabinetInfo)) {
		return false;
	}
	if (CabinetInfo->hasprev || CabinetInfo->hasnext) {
		return false;
	}

	FDIFree(CabinetInfo);
	CloseHandle(hf);

	if (!FDICopy(FDIContext, CABFilePart, CABPathPart, 0, FDINotify, NULL, NULL)) {
		return false;
	}

	// Destroy FDI context
	FDIDestroy(FDIContext);

	// Cleanup and return
	FDIFree(FDIErf);

    FDIFree(TargetDirectoryName);
	return true;
}
