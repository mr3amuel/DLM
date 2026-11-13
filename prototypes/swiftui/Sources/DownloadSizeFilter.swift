import SwiftUI

struct DownloadSizeFilter: View {
    @Binding var options: DownloadListOptions
    @Environment(\.dismiss) private var dismiss
    @State private var minimum = ""
    @State private var maximum = ""
    @State private var error: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("File Size").font(.headline)
            TextField("Minimum size (MB)", text: $minimum).textFieldStyle(.roundedBorder)
            TextField("Maximum size (MB)", text: $maximum).textFieldStyle(.roundedBorder)
            if let error { Text(error).font(.caption).foregroundStyle(DLMDesign.failure) }
            HStack {
                Button("Clear") { minimum = ""; maximum = ""; apply() }
                Spacer()
                Button("Cancel") { dismiss() }.keyboardShortcut(.cancelAction)
                Button("Apply") { apply() }.keyboardShortcut(.defaultAction)
            }
        }
        .padding(DLMDesign.inset).frame(width: 330).background(DLMDesign.canvas)
        .onAppear {
            minimum = NativeNumber.text(options.minimumBytes, places: 6)
            maximum = NativeNumber.text(options.maximumBytes, places: 6)
        }
    }

    private func apply() {
        do {
            var updated = options
            try updated.setSizeBounds(minimum: minimum, maximum: maximum)
            options = updated
            dismiss()
        } catch { self.error = error.localizedDescription }
    }
}
