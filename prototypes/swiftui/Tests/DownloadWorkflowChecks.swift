import Foundation

// UI-independent checks of the native draft, DTO and navigation contracts.
@MainActor
enum DownloadWorkflowChecks {
    static func run(model: DownloadModel) async throws {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        let noHeaders = try decoder.decode(SourceMetadata.self,
            from: Data(#"{"url":"https://example.invalid/file"}"#.utf8))
        precondition(noHeaders.total == nil && noHeaders.etag == nil && noHeaders.contentType == nil)
        var draft = Draft(url: " https://example.invalid/file ", filename: "")
        draft.source = noHeaders
        precondition(draft.prepared.filename == "file" && draft.prepared.source == noHeaders)
        draft.url = "https://example.invalid/other"
        precondition(draft.prepared.source == nil, "Carried metadata across different URLs")
        draft.filename = " renamed.zip "
        precondition(draft.prepared.filename == "renamed.zip")

        let wire = JSONEncoder()
        wire.keyEncodingStrategy = .convertToSnakeCase
        draft.source = SourceMetadata(url: draft.url, categoryId: "videos", contentType: "video/mp4", total: 4096, etag: "\"strong\"")
        let encoded = try JSONSerialization.jsonObject(with: wire.encode(draft)) as! [String: Any]
        let source = encoded["source"] as! [String: Any]
        precondition(source["content_type"] as? String == "video/mp4" && source["total"] as? Int == 4096)
        let review = try await model.inspect(draft)
        precondition(review.directory == "/resolved/Videos" && review.categoryId == "videos")
        precondition(review.duplicates.map(\.id) == ["existing-id", ""])
        try await model.locateExistingDownload(review.duplicates[0].id)
        precondition(model.selection == "existing-id" && model.requestedDownloadID == "existing-id")
        model.finishLocatingDownload()
        do {
            try await model.locateExistingDownload("")
            fatalError("Navigated to a filesystem-only collision")
        } catch let failure as EngineFailure { precondition(failure.code == "not_found") }

        lifecycle_fail(Int32(TEST_SOURCE), 1)
        do {
            _ = try await model.inspectSource(url: draft.url, filename: draft.filename)
            fatalError("Ignored a metadata failure")
        } catch let failure as EngineFailure { precondition(failure.code == "fixture") }
        // A failed optional read does not strand the source-inspection task or disable commands.
        let metadata = try await model.inspectSource(url: "https://example.invalid/file", filename: "file")
        precondition(metadata.total == nil && model.ready)
        for outcome in LinkVerification.Outcome.allCases {
            let result = try decoder.decode(LinkVerification.self, from: Data(
                "{\"outcome\":\"\(outcome.rawValue)\",\"reason\":\"fixture\",\"token\":\"ticket\",\"compared_bytes\":123}".utf8))
            precondition(result.safeToResume == (outcome == .safeToResume))
        }
        let absent = try decoder.decode(PartialFileInfo.self, from: Data(
            #"{"exists":false,"retained_verified_bytes":0}"#.utf8))
        precondition(!absent.exists && absent.fileLength == nil && absent.retainedVerifiedBytes == 0)
        print("Native download workflow checks passed: draft metadata, destination/duplicates, existing selection, recovery outcomes")
    }
}
